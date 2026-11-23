use std::sync::atomic::{AtomicU8, Ordering};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Lang {
    En = 0,
    Ja = 1,
}

static CURRENT_LANG: AtomicU8 = AtomicU8::new(0);

#[link(name = "kernel32")]
extern "system" {
    fn GetUserDefaultUILanguage() -> u16;
}

impl Lang {
    pub fn detect_os_default() -> Self {
        let lang_id = unsafe { GetUserDefaultUILanguage() };
        // Primary language ID 0x11 == LANG_JAPANESE
        if (lang_id & 0x03FF) == 0x11 {
            Lang::Ja
        } else {
            Lang::En
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Lang::En => "en",
            Lang::Ja => "ja",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "en" | "english" => Some(Lang::En),
            "ja" | "jp" | "japanese" => Some(Lang::Ja),
            _ => None,
        }
    }
}

pub fn set_lang(lang: Lang) {
    CURRENT_LANG.store(lang as u8, Ordering::SeqCst);
}

pub fn lang() -> Lang {
    match CURRENT_LANG.load(Ordering::SeqCst) {
        1 => Lang::Ja,
        _ => Lang::En,
    }
}

/// Concise bilingual lookup helper: `tr("English", "日本語")`.
#[inline]
pub fn tr(en: &'static str, ja: &'static str) -> &'static str {
    match lang() {
        Lang::En => en,
        Lang::Ja => ja,
    }
}
