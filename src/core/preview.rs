use std::fs::File;
use std::io::{self, Read};
use std::path::Path;

pub const MAX_TEXT_PREVIEW_BYTES: usize = 256 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextPreview {
    pub text: String,
    pub truncated: bool,
}

pub fn supports_text_preview(path: &Path) -> bool {
    if path
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| {
            matches!(
                name.to_ascii_lowercase().as_str(),
                "makefile" | "dockerfile" | "containerfile" | ".gitignore" | ".editorconfig"
            )
        })
    {
        return true;
    }
    let Some(extension) = path.extension().and_then(|value| value.to_str()) else {
        return false;
    };
    matches!(
        extension.to_ascii_lowercase().as_str(),
        "txt"
            | "md"
            | "json"
            | "json5"
            | "xml"
            | "ini"
            | "csv"
            | "log"
            | "toml"
            | "conf"
            | "properties"
            | "env"
            | "yaml"
            | "yml"
            | "rs"
            | "c"
            | "h"
            | "cc"
            | "cpp"
            | "hpp"
            | "cs"
            | "java"
            | "kt"
            | "kts"
            | "swift"
            | "dart"
            | "rb"
            | "php"
            | "lua"
            | "pl"
            | "r"
            | "js"
            | "jsx"
            | "ts"
            | "tsx"
            | "py"
            | "go"
            | "html"
            | "vue"
            | "svelte"
            | "graphql"
            | "proto"
            | "gradle"
            | "css"
            | "scss"
            | "sql"
            | "sh"
            | "ps1"
            | "bat"
            | "cmd"
            | "make"
    )
}

pub fn supports_image_preview(path: &Path) -> bool {
    let Some(extension) = path.extension().and_then(|value| value.to_str()) else {
        return false;
    };
    matches!(
        extension.to_ascii_lowercase().as_str(),
        "png" | "jpg" | "jpeg" | "bmp" | "gif"
    )
}

pub fn supports_preview(path: &Path) -> bool {
    supports_text_preview(path) || supports_image_preview(path)
}

/// Читает только ограниченный префикс текстового файла.
pub fn read_text_preview(path: &Path) -> io::Result<TextPreview> {
    let mut bytes = Vec::with_capacity(MAX_TEXT_PREVIEW_BYTES + 1);
    File::open(path)?
        .take((MAX_TEXT_PREVIEW_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    let truncated = bytes.len() > MAX_TEXT_PREVIEW_BYTES;
    bytes.truncate(MAX_TEXT_PREVIEW_BYTES);
    Ok(TextPreview {
        text: decode_text(&bytes),
        truncated,
    })
}

fn decode_text(bytes: &[u8]) -> String {
    if let Some(body) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return String::from_utf8_lossy(body).into_owned();
    }
    if let Some(body) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        return decode_utf16(body, true);
    }
    if let Some(body) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        return decode_utf16(body, false);
    }
    String::from_utf8_lossy(bytes).into_owned()
}

fn decode_utf16(bytes: &[u8], little_endian: bool) -> String {
    let units = bytes.chunks_exact(2).map(|pair| {
        if little_endian {
            u16::from_le_bytes([pair[0], pair[1]])
        } else {
            u16::from_be_bytes([pair[0], pair[1]])
        }
    });
    char::decode_utf16(units)
        .map(|character| character.unwrap_or(char::REPLACEMENT_CHARACTER))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_file(name: &str, contents: &[u8]) -> PathBuf {
        let id = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("filemanager-preview-{id}-{name}"));
        std::fs::write(&path, contents).unwrap();
        path
    }

    #[test]
    fn reads_only_the_preview_limit_and_marks_truncation() {
        let contents = vec![b'x'; MAX_TEXT_PREVIEW_BYTES + 512];
        let path = temp_file("large.txt", &contents);
        let preview = read_text_preview(&path).unwrap();
        assert!(preview.truncated);
        assert_eq!(preview.text.len(), MAX_TEXT_PREVIEW_BYTES);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn decodes_utf8_and_bom_marked_utf16() {
        let utf8 = temp_file("utf8.txt", b"\xEF\xBB\xBFhello");
        assert_eq!(read_text_preview(&utf8).unwrap().text, "hello");

        let utf16 = temp_file("utf16.txt", &[0xFF, 0xFE, b'h', 0, b'i', 0]);
        assert_eq!(read_text_preview(&utf16).unwrap().text, "hi");
        std::fs::remove_file(utf8).unwrap();
        std::fs::remove_file(utf16).unwrap();
    }

    #[test]
    fn recognizes_text_and_code_extensions_case_insensitively() {
        assert!(supports_text_preview(Path::new("Readme.MD")));
        assert!(supports_text_preview(Path::new("main.RS")));
        assert!(supports_text_preview(Path::new("Dockerfile")));
        assert!(!supports_text_preview(Path::new("photo.png")));
        assert!(supports_image_preview(Path::new("motion.GIF")));
        assert!(!supports_image_preview(Path::new("photo.webp")));
    }
}
