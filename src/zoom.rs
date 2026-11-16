pub const BASE_PT: i32 = 11;
pub const MIN_PT: i32 = 8;
pub const MAX_PT: i32 = 28;

pub fn clamp_pt(pt: i32) -> i32 {
    pt.clamp(MIN_PT, MAX_PT)
}

pub fn step(pt: i32, delta: i32) -> i32 {
    clamp_pt(pt.saturating_add(delta))
}

pub fn webkit_factor(pt: i32) -> f64 {
    f64::from(clamp_pt(pt)) / f64::from(BASE_PT)
}

/// Масштаб окна: дерево, меню, кнопки. Не трогает редактор — иначе GTK
/// падает на скрытых строках таблицы при смене font-size у textview.
pub fn ui_css(pt: i32) -> String {
    let pt = clamp_pt(pt);
    format!(
        "window.verst, window.verst popover, window.verst menu,
         window.verst label, window.verst button, window.verst entry,
         window.verst listview, window.verst columnview {{
            font-size: {pt}pt;
        }}"
    )
}

pub fn text_css(pt: i32) -> String {
    let pt = clamp_pt(pt);
    format!(
        "entry.tw-md-cell, entry.tw-md-cell > text {{
            font-size: {pt}pt;
        }}
        gutterrenderertext.tw-line-numbers {{
            font-family: monospace;
            font-size: {pt}pt;
        }}"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zoom_stays_inside_limits() {
        assert_eq!(step(MIN_PT, -1), MIN_PT);
        assert_eq!(step(MAX_PT, 1), MAX_PT);
        assert_eq!(step(BASE_PT, 1), BASE_PT + 1);
    }

    #[test]
    fn webkit_factor_is_one_at_base() {
        assert!((webkit_factor(BASE_PT) - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn ui_css_does_not_restyle_textview() {
        let css = ui_css(14);
        assert!(css.contains("14pt"));
        assert!(!css.contains("textview"));
        let text = text_css(16);
        assert!(text.contains("16pt"));
        assert!(!text.contains("textview"));
    }
}
