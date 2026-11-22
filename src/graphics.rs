//! Выбор способа отрисовки GTK, когда в системе нет аппаратного 3D.
//!
//! Без 3D-ускорения (виртуальная машина, серверная видеокарта) GTK 4 рисует
//! через Vulkan на программном драйвере lavapipe. Он компилирует шейдеры
//! через LLVM прямо на процессоре, и первый кадр выходит почти вдвое
//! дольше, чем у отрисовки через cairo. Замер на виртуальной машине с virtio-gpu:
//! первый кадр 776 мс через Vulkan против 397 мс через cairo.

use std::path::Path;

/// Видеоустройство из /sys/class/drm.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Card {
    /// Имя драйвера ядра: i915, amdgpu, virtio_gpu, simpledrm…
    pub driver: String,
    /// Для virtio-gpu: включено ли 3D (virgl). Для остальных — None.
    pub virgl: Option<bool>,
}

/// Драйверы, у которых нет 3D: только кадровый буфер или 2D.
const SOFTWARE_DRIVERS: &[&str] = &[
    "simpledrm",
    "simple-framebuffer",
    "efifb",
    "vesafb",
    "bochs-drm",
    "bochs",
    "cirrus",
    "cirrus-qemu",
    "qxl",
    "vboxvideo",
    "hyperv_drm",
    "ast",
    "mgag200",
    "udl",
];

/// Нет ни одного устройства с аппаратным 3D. Пустой список — не известно,
/// значит, решения нет: ничего не меняем.
pub fn software_only(cards: &[Card]) -> bool {
    if cards.is_empty() {
        return false;
    }
    cards.iter().all(|card| match card.driver.as_str() {
        "virtio_gpu" | "virtio-gpu" => card.virgl != Some(true),
        name => SOFTWARE_DRIVERS.contains(&name),
    })
}

/// Видеоустройства из /sys/class/drm. None, если каталог не читается.
pub fn scan_cards() -> Option<Vec<Card>> {
    let entries = std::fs::read_dir("/sys/class/drm").ok()?;
    let mut cards = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        // card1, но не разъёмы вида card1-Virtual-1 и не renderD128.
        if !name.starts_with("card") || name.contains('-') {
            continue;
        }
        let Ok(device) = std::fs::canonicalize(entry.path().join("device")) else {
            continue;
        };
        let Some(driver) = driver_of(&device) else {
            continue;
        };
        if driver == "virtio-pci" {
            // Сам видеодрайвер висит ниже: …/virtio0/driver -> virtio_gpu.
            if let Some(card) = virtio_child(&device) {
                cards.push(card);
            }
            continue;
        }
        let virgl = (driver == "virtio_gpu").then(|| virgl_enabled(&device));
        cards.push(Card { driver, virgl });
    }
    Some(cards)
}

fn driver_of(device: &Path) -> Option<String> {
    let link = std::fs::read_link(device.join("driver")).ok()?;
    Some(link.file_name()?.to_string_lossy().to_string())
}

fn virtio_child(device: &Path) -> Option<Card> {
    for entry in std::fs::read_dir(device).ok()?.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.starts_with("virtio") {
            continue;
        }
        if driver_of(&path).as_deref() == Some("virtio_gpu") {
            return Some(Card {
                driver: "virtio_gpu".to_string(),
                virgl: Some(virgl_enabled(&path)),
            });
        }
    }
    None
}

/// Бит 0 в features у virtio-gpu — VIRTIO_GPU_F_VIRGL (3D).
fn virgl_enabled(device: &Path) -> bool {
    std::fs::read_to_string(device.join("features"))
        .map(|bits| virgl_bit(&bits))
        .unwrap_or(false)
}

fn virgl_bit(features: &str) -> bool {
    features.trim().starts_with('1')
}

static SOFTWARE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();

/// В системе нет аппаратного 3D. Считается один раз за запуск.
pub fn software_graphics() -> bool {
    *SOFTWARE.get_or_init(|| scan_cards().is_some_and(|cards| software_only(&cards)))
}

/// Если пользователь не задал GSK_RENDERER и аппаратного 3D нет, выбрать cairo.
/// Вызывать до создания приложения GTK. Возвращает, был ли выбран cairo.
pub fn prefer_cairo_without_gpu() -> bool {
    if std::env::var_os("GSK_RENDERER").is_some() {
        return false;
    }
    let software = software_graphics();
    if software {
        std::env::set_var("GSK_RENDERER", "cairo");
    }
    software
}

/// Без аппаратного 3D WebKit рисует предпросмотр через программный OpenGL
/// (llvmpipe): процесс страницы держит LLVM и около 180 МБ частной памяти.
/// Без GPU-композитинга — около 60 МБ, страница рисуется процессором.
/// Настройка WebView hardware-acceleration-policy в WebKitGTK 6 этого
/// не выключает, поэтому переменная окружения. Вызывать до создания WebView.
pub fn lighten_webkit_without_gpu() -> bool {
    if std::env::var_os("WEBKIT_DISABLE_COMPOSITING_MODE").is_some() || !software_graphics() {
        return false;
    }
    std::env::set_var("WEBKIT_DISABLE_COMPOSITING_MODE", "1");
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card(driver: &str, virgl: Option<bool>) -> Card {
        Card {
            driver: driver.to_string(),
            virgl,
        }
    }

    #[test]
    fn virtio_without_virgl_is_software() {
        assert!(software_only(&[card("virtio_gpu", Some(false))]));
        assert!(!software_only(&[card("virtio_gpu", Some(true))]));
    }

    #[test]
    fn real_gpu_keeps_default_renderer() {
        assert!(!software_only(&[card("i915", None)]));
        assert!(!software_only(&[card("amdgpu", None)]));
        assert!(!software_only(&[card("nvidia", None)]));
        // Встроенный кадровый буфер рядом с настоящей видеокартой — не повод.
        assert!(!software_only(&[
            card("simpledrm", None),
            card("amdgpu", None)
        ]));
    }

    #[test]
    fn framebuffer_only_is_software() {
        assert!(software_only(&[card("simpledrm", None)]));
        assert!(software_only(&[card("qxl", None), card("bochs-drm", None)]));
    }

    #[test]
    fn unknown_or_empty_changes_nothing() {
        assert!(!software_only(&[]));
        assert!(!software_only(&[card("something_new", None)]));
    }

    #[test]
    fn virgl_is_bit_zero() {
        assert!(!virgl_bit("0100000000000000000000000000110010000000"));
        assert!(virgl_bit("1100000000000000000000000000110010000000\n"));
    }
}
