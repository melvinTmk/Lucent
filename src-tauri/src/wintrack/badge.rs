//! Extracts notification badge indicators from window titles (e.g. "(3) Discord").

fn is_valid_badge(s: &str) -> bool {
    let s = s.trim();
    if s.is_empty() {
        return false;
    }
    if s == "*" || s == "•" {
        return true;
    }
    // Check if it's numbers, possibly ending or starting with '+'
    let num_part = s.trim_matches('+').trim();
    if num_part.is_empty() || !num_part.chars().all(|c| c.is_ascii_digit()) {
        return false;
    }
    // If it's a 4-digit number between 1900 and 2099 without a '+', it's almost certainly a year (e.g. "(2024)")
    if !s.contains('+') && num_part.len() == 4 {
        if let Ok(year) = num_part.parse::<u32>() {
            if (1900..=2099).contains(&year) {
                return false;
            }
        }
    }
    true
}

fn format_badge(s: &str) -> String {
    let s = s.trim();
    if s == "*" || s == "•" {
        "•".to_string()
    } else {
        s.to_string()
    }
}

/// Extracts a notification badge string (e.g. "3", "99+", "•") from a window title.
/// Returns `None` if no notification pattern is detected.
pub fn extract_badge(title: &str) -> Option<String> {
    let chars: Vec<(usize, char)> = title.char_indices().collect();
    for (i, &(start_idx, c)) in chars.iter().enumerate() {
        let close = match c {
            '(' => ')',
            '[' => ']',
            _ => continue,
        };
        // Find closing matching bracket
        if let Some(&(end_idx, _)) = chars[i + 1..].iter().find(|&&(_, ch)| ch == close) {
            let inner = title[start_idx + 1..end_idx].trim();
            if is_valid_badge(inner) {
                return Some(format_badge(inner));
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_parenthesized_and_bracketed_counts() {
        assert_eq!(extract_badge("(3) Discord | #general"), Some("3".into()));
        assert_eq!(extract_badge("(1) WhatsApp"), Some("1".into()));
        assert_eq!(extract_badge("(99+) WhatsApp"), Some("99+".into()));
        assert_eq!(extract_badge("[5] Slack"), Some("5".into()));
        assert_eq!(extract_badge("Inbox (12) - Mail"), Some("12".into()));
        assert_eq!(extract_badge("Telegram (42)"), Some("42".into()));
    }

    #[test]
    fn extracts_unread_dots_and_asterisks() {
        assert_eq!(extract_badge("(*) Discord"), Some("•".into()));
        assert_eq!(extract_badge("(•) Slack"), Some("•".into()));
    }

    #[test]
    fn ignores_non_notification_patterns() {
        assert_eq!(extract_badge("Call of Duty (2024)"), None);
        assert_eq!(extract_badge("Game (1998)"), None);
        assert_eq!(extract_badge("Document (Copy)"), None);
        assert_eq!(extract_badge("Process Explorer (64-bit)"), None);
        assert_eq!(extract_badge("Installer (v1.2.3)"), None);
        assert_eq!(extract_badge("Normal Window Title"), None);
        assert_eq!(extract_badge(""), None);
    }
}
