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

pub fn css(pt: i32) -> String {
    let pt = clamp_pt(pt);
    format!(
        "window, popover, menu, label, button, entry, text, textview, listview {{
            font-size: {pt}pt;
        }}
        textview, textview text {{
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
}
