use std::collections::{BTreeSet, HashSet};
use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use super::fs::{self as file_system, FileEntry, ReadDirError};

const CANCEL_CHECK_INTERVAL: usize = 256;
const MAX_RECURSION_DEPTH: usize = 128;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SearchOptions {
    pub query: String,
    pub recursive: bool,
    pub fuzzy: bool,
    pub extensions: BTreeSet<String>,
    pub min_size: Option<u64>,
    pub max_size: Option<u64>,
    pub modified_from: Option<SystemTime>,
    pub modified_before: Option<SystemTime>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SearchValidationError {
    InvalidSize,
    SizeRange,
    InvalidDate,
    DateRange,
}

pub fn parse_options(
    query: &str,
    recursive: bool,
    fuzzy: bool,
    extensions: &str,
    min_size: &str,
    max_size: &str,
    modified_from: &str,
    modified_to: &str,
) -> Result<SearchOptions, SearchValidationError> {
    let min_size = parse_size(min_size)?;
    let max_size = parse_size(max_size)?;
    if min_size.zip(max_size).is_some_and(|(min, max)| min > max) {
        return Err(SearchValidationError::SizeRange);
    }

    let modified_from = parse_date(modified_from, false)?;
    let modified_before = parse_date(modified_to, true)?;
    if modified_from
        .zip(modified_before)
        .is_some_and(|(from, before)| from >= before)
    {
        return Err(SearchValidationError::DateRange);
    }

    let extensions = extensions
        .split([',', ';', ' ', '\t'])
        .filter_map(|extension| {
            let extension = extension.trim().trim_start_matches('.').to_lowercase();
            (!extension.is_empty()).then_some(extension)
        })
        .collect();

    Ok(SearchOptions {
        query: query.trim().to_lowercase(),
        recursive,
        fuzzy,
        extensions,
        min_size,
        max_size,
        modified_from,
        modified_before,
    })
}

fn parse_size(value: &str) -> Result<Option<u64>, SearchValidationError> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(None);
    }

    let split_at = value
        .find(|ch: char| !ch.is_ascii_digit() && ch != '.')
        .unwrap_or(value.len());
    let (number, unit) = value.split_at(split_at);
    let number = number
        .trim()
        .parse::<f64>()
        .map_err(|_| SearchValidationError::InvalidSize)?;
    if !number.is_finite() || number < 0.0 {
        return Err(SearchValidationError::InvalidSize);
    }

    let multiplier = match unit.trim().to_lowercase().as_str() {
        "" | "b" | "байт" | "байта" | "байтов" => 1.0,
        "k" | "kb" | "kib" | "к" | "кб" | "киб" => 1024.0,
        "m" | "mb" | "mib" | "м" | "мб" | "миб" => 1024.0_f64.powi(2),
        "g" | "gb" | "gib" | "г" | "гб" | "гиб" => 1024.0_f64.powi(3),
        _ => return Err(SearchValidationError::InvalidSize),
    };
    let bytes = number * multiplier;
    if bytes > u64::MAX as f64 {
        return Err(SearchValidationError::InvalidSize);
    }
    Ok(Some(bytes as u64))
}

/// Разбирает календарную дату как полночь UTC; верхняя граница задаёт
/// полночь следующего дня, чтобы использовать полуоткрытый интервал.
fn parse_date(value: &str, upper_bound: bool) -> Result<Option<SystemTime>, SearchValidationError> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(None);
    }
    let mut parts = value.split('-');
    let (Some(year), Some(month), Some(day)) = (parts.next(), parts.next(), parts.next()) else {
        return Err(SearchValidationError::InvalidDate);
    };
    if parts.next().is_some() || year.len() != 4 || month.len() != 2 || day.len() != 2 {
        return Err(SearchValidationError::InvalidDate);
    }
    let (Ok(year), Ok(month), Ok(day)) = (
        year.parse::<i64>(),
        month.parse::<i64>(),
        day.parse::<i64>(),
    ) else {
        return Err(SearchValidationError::InvalidDate);
    };
    if !(1..=12).contains(&month) || year < 1601 {
        return Err(SearchValidationError::InvalidDate);
    }
    let days_in_month = match month {
        2 if is_leap_year(year) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    };
    if !(1..=days_in_month).contains(&day) {
        return Err(SearchValidationError::InvalidDate);
    }
    let days = days_from_civil(year, month, day)
        .ok_or(SearchValidationError::InvalidDate)?
        .checked_add(if upper_bound { 1 } else { 0 })
        .ok_or(SearchValidationError::InvalidDate)?;
    let seconds = u64::try_from(days)
        .ok()
        .and_then(|days| days.checked_mul(24 * 60 * 60))
        .ok_or(SearchValidationError::InvalidDate)?;
    Ok(Some(UNIX_EPOCH + Duration::from_secs(seconds)))
}

fn is_leap_year(year: i64) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

fn days_from_civil(year: i64, month: i64, day: i64) -> Option<i64> {
    let adjusted_year = if month <= 2 {
        year.checked_sub(1)?
    } else {
        year
    };
    let era = adjusted_year.div_euclid(400);
    let year_of_era = adjusted_year - era * 400;
    let adjusted_month = month + if month > 2 { -3 } else { 9 };
    let day_of_year = (153 * adjusted_month + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era.checked_mul(146_097)?
        .checked_add(day_of_era)?
        .checked_sub(719_468)
}

/// Ищет в папке и при необходимости обходит реальные подпапки.
/// Вызывать в рабочем потоке; отмена проверяется между пакетами записей.
pub fn search(
    root: &Path,
    options: &SearchOptions,
    cancel: &AtomicBool,
) -> Result<Vec<FileEntry>, ReadDirError> {
    let mut pending = vec![(root.to_path_buf(), 0usize)];
    let mut results = Vec::new();
    let mut scanned = 0usize;
    let mut visited = HashSet::new();
    if let Ok(root) = fs::canonicalize(root) {
        visited.insert(root);
    }

    while let Some((directory, depth)) = pending.pop() {
        if cancel.load(Ordering::Relaxed) {
            return Err(ReadDirError::Cancelled);
        }
        let items = match fs::read_dir(&directory) {
            Ok(items) => items,
            Err(_) if directory != root => continue,
            Err(error) => return Err(ReadDirError::Io(error)),
        };

        for item in items {
            scanned += 1;
            if scanned % CANCEL_CHECK_INTERVAL == 0 && cancel.load(Ordering::Relaxed) {
                return Err(ReadDirError::Cancelled);
            }
            let entry = item.map_err(ReadDirError::Io)?;
            let Some(mapped) = file_system::map_entry(&entry) else {
                continue;
            };
            if options.recursive
                && mapped.is_dir
                && !mapped.is_symlink
                && depth < MAX_RECURSION_DEPTH
            {
                if let Ok(canonical) = fs::canonicalize(&mapped.path)
                    && visited.insert(canonical)
                {
                    pending.push((mapped.path.clone(), depth + 1));
                }
            }
            if matches_entry(&mapped, options) {
                results.push(mapped);
            }
        }
    }

    results.sort_by(|a, b| {
        let score_a = name_score(&a.name, &options.query, options.fuzzy);
        let score_b = name_score(&b.name, &options.query, options.fuzzy);
        score_b
            .cmp(&score_a)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    Ok(results)
}

pub(crate) fn matches_entry(entry: &FileEntry, options: &SearchOptions) -> bool {
    if name_score(&entry.name, &options.query, options.fuzzy).is_none() {
        return false;
    }
    if !options.extensions.is_empty()
        && (entry.is_dir
            || !entry
                .extension
                .as_ref()
                .is_some_and(|extension| options.extensions.contains(extension)))
    {
        return false;
    }
    if options.min_size.is_some() || options.max_size.is_some() {
        let Some(size) = entry.size else {
            return false;
        };
        if options.min_size.is_some_and(|min| size < min)
            || options.max_size.is_some_and(|max| size > max)
        {
            return false;
        }
    }
    if options.modified_from.is_some() || options.modified_before.is_some() {
        let Some(modified) = entry.modified else {
            return false;
        };
        if options.modified_from.is_some_and(|from| modified < from)
            || options
                .modified_before
                .is_some_and(|before| modified >= before)
        {
            return false;
        }
    }
    true
}

/// Возвращает оценку совпадения подпоследовательности или `None`.
/// Непрерывные совпадения и совпадения на границах слов получают больший балл.
pub fn name_score(name: &str, query: &str, fuzzy: bool) -> Option<i32> {
    let name = name.to_lowercase();
    let query = query.to_lowercase();
    if query.is_empty() {
        return Some(0);
    }
    if let Some(start) = name.find(&query) {
        return Some(1_000 - start as i32 - name.len() as i32 / 8);
    }
    if !fuzzy {
        return None;
    }

    let name_chars: Vec<char> = name.chars().collect();
    let query_chars: Vec<char> = query.chars().collect();
    let mut positions = Vec::with_capacity(query_chars.len());
    let mut next = 0usize;
    for wanted in query_chars {
        let Some(relative) = name_chars[next..].iter().position(|ch| *ch == wanted) else {
            return None;
        };
        let position = next + relative;
        positions.push(position);
        next = position + 1;
    }

    let mut score = 500 - name_chars.len() as i32;
    for (index, &position) in positions.iter().enumerate() {
        if index == 0 && position == 0 {
            score += 30;
        }
        if position == 0 || !name_chars[position - 1].is_alphanumeric() {
            score += 18;
        }
        if index > 0 {
            let previous = positions[index - 1];
            if position == previous + 1 {
                score += 24;
            } else {
                score -= (position - previous - 1) as i32 * 3;
            }
        }
    }
    Some(score)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::AtomicBool;

    fn temp_dir() -> PathBuf {
        let id = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("filemanager-search-{id}"));
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn fuzzy_score_prefers_contiguous_and_boundary_matches() {
        assert!(
            name_score("file_manager.rs", "fmr", true).unwrap()
                > name_score("somefarawaymanager.rs", "fmr", true).unwrap()
        );
        assert!(name_score("Report.txt", "rep", false).is_some());
        assert!(name_score("Report.txt", "rpt", false).is_none());
    }

    #[test]
    fn parses_size_units_and_rejects_inverted_ranges() {
        let options = parse_options("", false, false, "", "1 KB", "2 MiB", "", "").unwrap();
        assert_eq!(options.min_size, Some(1024));
        assert_eq!(options.max_size, Some(2 * 1024 * 1024));
        assert_eq!(
            parse_options("", false, false, "", "1 МБ", "", "", "")
                .unwrap()
                .min_size,
            Some(1024 * 1024)
        );
        assert_eq!(
            parse_options("", false, false, "", "2 MB", "1 MB", "", ""),
            Err(SearchValidationError::SizeRange)
        );
    }

    #[test]
    fn parses_inclusive_date_range() {
        let options =
            parse_options("", false, false, "", "", "", "2024-02-29", "2024-03-01").unwrap();
        let from = options.modified_from.unwrap();
        let before = options.modified_before.unwrap();
        assert_eq!(
            before.duration_since(from).unwrap().as_secs(),
            2 * 24 * 60 * 60
        );
        assert_eq!(
            parse_options("", false, false, "", "", "", "2023-02-29", ""),
            Err(SearchValidationError::InvalidDate)
        );
    }

    #[test]
    fn search_filters_extensions_and_recurses_only_when_requested() {
        let root = temp_dir();
        let nested = root.join("nested");
        fs::create_dir(&nested).unwrap();
        fs::write(root.join("Alpha.txt"), b"a").unwrap();
        fs::write(root.join("Beta.csv"), b"bb").unwrap();
        fs::write(nested.join("Gamma.txt"), b"ccc").unwrap();

        let cancel = AtomicBool::new(false);
        let options = parse_options("a", true, false, ".txt", "", "", "", "").unwrap();
        let results = search(&root, &options, &cancel).unwrap();
        assert_eq!(
            results
                .iter()
                .map(|entry| entry.name.as_str())
                .collect::<Vec<_>>(),
            vec!["Alpha.txt", "Gamma.txt"]
        );

        let options = parse_options("a", false, false, ".txt", "", "", "", "").unwrap();
        let results = search(&root, &options, &cancel).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "Alpha.txt");

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn search_observes_cancellation_before_scanning() {
        let root = temp_dir();
        let cancel = AtomicBool::new(true);
        assert!(matches!(
            search(&root, &SearchOptions::default(), &cancel),
            Err(ReadDirError::Cancelled)
        ));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn search_finds_a_match_in_a_folder_with_thousands_of_entries() {
        let root = temp_dir();
        for index in 0..1500 {
            fs::write(root.join(format!("item-{index:04}.txt")), b"x").unwrap();
        }
        let options = parse_options("item-1499", false, false, "", "", "", "", "").unwrap();
        let results = search(&root, &options, &AtomicBool::new(false)).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "item-1499.txt");
        fs::remove_dir_all(root).unwrap();
    }
}
