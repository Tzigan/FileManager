use std::io;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;

use ::windows::Win32::Foundation::GENERIC_READ;
use ::windows::Win32::Graphics::Imaging::{
    CLSID_WICImagingFactory, GUID_WICPixelFormat32bppRGBA, IWICImagingFactory, IWICPalette,
    WICBitmapDitherTypeNone, WICBitmapInterpolationModeFant, WICBitmapPaletteTypeCustom,
    WICDecodeMetadataCacheOnDemand,
};
use ::windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize,
};
use ::windows::core::{IUnknown, PCWSTR};

use crate::platform::DecodedImage;

const MAX_SOURCE_PIXELS: u64 = 40_000_000;
const MAX_OUTPUT_DIMENSION: u32 = 2048;

struct ComApartment;

impl ComApartment {
    fn initialize() -> io::Result<Self> {
        unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }
            .ok()
            .map_err(windows_error)?;
        Ok(Self)
    }
}

impl Drop for ComApartment {
    fn drop(&mut self) {
        unsafe { CoUninitialize() };
    }
}

/// Декодирует первый кадр WIC-совместимого изображения в ограниченный RGBA-эскиз.
pub fn decode_image(path: &Path, max_dimension: u32) -> io::Result<DecodedImage> {
    let _com = ComApartment::initialize()?;
    let factory: IWICImagingFactory = unsafe {
        CoCreateInstance(
            &CLSID_WICImagingFactory,
            None::<&IUnknown>,
            CLSCTX_INPROC_SERVER,
        )
    }
    .map_err(windows_error)?;
    let wide_path: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
    let decoder = unsafe {
        factory.CreateDecoderFromFilename(
            PCWSTR(wide_path.as_ptr()),
            None,
            GENERIC_READ,
            WICDecodeMetadataCacheOnDemand,
        )
    }
    .map_err(windows_error)?;
    let frame = unsafe { decoder.GetFrame(0) }.map_err(windows_error)?;
    let (mut width, mut height) = (0u32, 0u32);
    unsafe { frame.GetSize(&mut width, &mut height) }.map_err(windows_error)?;
    if width == 0 || height == 0 || u64::from(width) * u64::from(height) > MAX_SOURCE_PIXELS {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "image dimensions exceed the preview limit",
        ));
    }

    let limit = max_dimension.clamp(1, MAX_OUTPUT_DIMENSION);
    let scale = (f64::from(limit) / f64::from(width.max(height))).min(1.0);
    let output_width = (f64::from(width) * scale).round().max(1.0) as u32;
    let output_height = (f64::from(height) * scale).round().max(1.0) as u32;
    let scaler = unsafe { factory.CreateBitmapScaler() }.map_err(windows_error)?;
    unsafe {
        scaler.Initialize(
            &frame,
            output_width,
            output_height,
            WICBitmapInterpolationModeFant,
        )
    }
    .map_err(windows_error)?;

    let converter = unsafe { factory.CreateFormatConverter() }.map_err(windows_error)?;
    unsafe {
        converter.Initialize(
            &scaler,
            &GUID_WICPixelFormat32bppRGBA,
            WICBitmapDitherTypeNone,
            None::<&IWICPalette>,
            0.0,
            WICBitmapPaletteTypeCustom,
        )
    }
    .map_err(windows_error)?;

    let stride = output_width
        .checked_mul(4)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "image row is too large"))?;
    let buffer_size = (stride as usize)
        .checked_mul(output_height as usize)
        .filter(|size| *size <= 32 * 1024 * 1024)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "image preview is too large"))?;
    let mut rgba = vec![0; buffer_size];
    unsafe { converter.CopyPixels(std::ptr::null(), stride, &mut rgba) }.map_err(windows_error)?;

    Ok(DecodedImage {
        width: output_width,
        height: output_height,
        rgba,
    })
}

fn windows_error(error: ::windows::core::Error) -> io::Error {
    io::Error::other(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_a_small_bmp_to_rgba() {
        let path = std::env::temp_dir().join(format!(
            "filemanager-wic-{}-{}.bmp",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let mut bmp = vec![0u8; 58];
        bmp[0..2].copy_from_slice(b"BM");
        bmp[2..6].copy_from_slice(&58u32.to_le_bytes());
        bmp[10..14].copy_from_slice(&54u32.to_le_bytes());
        bmp[14..18].copy_from_slice(&40u32.to_le_bytes());
        bmp[18..22].copy_from_slice(&1i32.to_le_bytes());
        bmp[22..26].copy_from_slice(&1i32.to_le_bytes());
        bmp[26..28].copy_from_slice(&1u16.to_le_bytes());
        bmp[28..30].copy_from_slice(&24u16.to_le_bytes());
        bmp[34..38].copy_from_slice(&4u32.to_le_bytes());
        bmp[54..58].copy_from_slice(&[0, 0, 255, 0]);
        std::fs::write(&path, bmp).unwrap();

        let decoded = decode_image(&path, 128).unwrap();
        assert_eq!((decoded.width, decoded.height), (1, 1));
        assert_eq!(decoded.rgba, [255, 0, 0, 255]);
        std::fs::remove_file(path).unwrap();
    }
}
