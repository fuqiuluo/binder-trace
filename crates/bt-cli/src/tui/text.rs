pub(super) fn fit(text: &str, width: usize) -> String {
    let mut result = truncate_with_ellipsis(text, width);
    let visible = display_width(&result);
    if visible < width {
        result.push_str(&" ".repeat(width - visible));
    }
    result
}

pub(super) fn fit_right(text: &str, width: usize) -> String {
    let result = truncate_with_ellipsis(text, width);
    let visible = display_width(&result);
    if visible < width {
        format!("{}{}", " ".repeat(width - visible), result)
    } else {
        result
    }
}

pub(super) fn truncate_with_ellipsis(text: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }

    if display_width(text) <= width {
        return text.to_owned();
    }

    if width <= 3 {
        return ".".repeat(width);
    }

    let mut result = String::new();
    let mut used = 0;
    for ch in text.chars() {
        let ch_width = char_display_width(ch);
        if used + ch_width > width - 3 {
            break;
        }
        result.push(ch);
        used += ch_width;
    }
    result.push_str("...");
    result
}

pub(super) fn display_width(text: &str) -> usize {
    text.chars().map(char_display_width).sum()
}

fn char_display_width(ch: char) -> usize {
    if ch.is_control() {
        0
    } else if is_east_asian_wide(ch) {
        2
    } else {
        1
    }
}

fn is_east_asian_wide(ch: char) -> bool {
    matches!(
        ch,
        '\u{1100}'..='\u{115f}'
            | '\u{2329}'..='\u{232a}'
            | '\u{2e80}'..='\u{a4cf}'
            | '\u{ac00}'..='\u{d7a3}'
            | '\u{f900}'..='\u{faff}'
            | '\u{fe10}'..='\u{fe19}'
            | '\u{fe30}'..='\u{fe6f}'
            | '\u{ff00}'..='\u{ff60}'
            | '\u{ffe0}'..='\u{ffe6}'
            | '\u{20000}'..='\u{2fffd}'
            | '\u{30000}'..='\u{3fffd}'
    )
}
