use std::path::Path;

/// Как показать файл. Текстовые открываются в редакторе.
/// Остальные только смотрим тем, что уже есть в GTK и WebKit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shown {
    Text,
    Html,
    /// PNG, JPEG, BMP, TIFF, ICO. Их читает GdkPixbuf из GTK.
    Picture,
    /// GIF и WebP. Их рисует уже подключённый WebKit.
    WebImage,
    Pdf,
    Audio,
    Video,
    /// SVG. Исходник остаётся текстом, картинку рисует WebKit.
    Svg,
}

pub fn shown(path: &Path) -> Shown {
    let Some(ext) = extension(path) else {
        return Shown::Text;
    };
    match ext.as_str() {
        "html" | "htm" | "xhtml" => Shown::Html,
        "png" | "jpg" | "jpeg" | "bmp" | "tif" | "tiff" | "ico" => Shown::Picture,
        "gif" | "webp" => Shown::WebImage,
        "pdf" => Shown::Pdf,
        "mp3" | "ogg" | "oga" | "opus" | "flac" | "wav" | "spx" => Shown::Audio,
        "webm" => Shown::Video,
        "svg" => Shown::Svg,
        _ => Shown::Text,
    }
}

pub fn is_text(path: &Path) -> bool {
    matches!(shown(path), Shown::Text | Shown::Html | Shown::Svg)
}

pub fn is_view_only(path: &Path) -> bool {
    !is_text(path)
}

fn extension(path: &Path) -> Option<String> {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.to_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pictures_html_pdf_and_media_are_recognized() {
        assert_eq!(shown(Path::new("a.PNG")), Shown::Picture);
        assert_eq!(shown(Path::new("a.jpeg")), Shown::Picture);
        assert_eq!(shown(Path::new("a.tiff")), Shown::Picture);
        assert_eq!(shown(Path::new("a.gif")), Shown::WebImage);
        assert_eq!(shown(Path::new("a.webp")), Shown::WebImage);
        assert_eq!(shown(Path::new("page.HTML")), Shown::Html);
        assert_eq!(shown(Path::new("doc.pdf")), Shown::Pdf);
        assert_eq!(shown(Path::new("song.mp3")), Shown::Audio);
        assert_eq!(shown(Path::new("talk.ogg")), Shown::Audio);
        assert_eq!(shown(Path::new("clip.webm")), Shown::Video);
        assert_eq!(shown(Path::new("icon.svg")), Shown::Svg);
        assert!(is_text(Path::new("note.md")));
        assert!(is_text(Path::new("page.html")));
        assert!(is_text(Path::new("icon.svg")));
        assert!(!is_view_only(Path::new("icon.svg")));
        assert!(is_view_only(Path::new("photo.jpg")));
        assert!(!is_view_only(Path::new("note.md")));
    }
}
