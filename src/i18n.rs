use std::sync::atomic::{AtomicU8, Ordering};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Lang {
    En = 0,
    Ja = 1,
}

static CURRENT_LANG: AtomicU8 = AtomicU8::new(0);

#[cfg(windows)]
#[link(name = "kernel32")]
extern "system" {
    fn GetUserDefaultUILanguage() -> u16;
}

#[cfg(target_os = "macos")]
mod macos_lang {
    use std::ffi::c_void;

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFLocaleCopyPreferredLanguages() -> *const c_void;
        fn CFArrayGetCount(the_array: *const c_void) -> isize;
        fn CFArrayGetValueAtIndex(the_array: *const c_void, idx: isize) -> *const c_void;
        fn CFStringGetCString(
            the_string: *const c_void,
            buffer: *mut i8,
            buffer_size: isize,
            encoding: u32,
        ) -> u8;
        fn CFRelease(cf: *const c_void);
    }

    pub fn is_japanese() -> bool {
        const K_CF_STRING_ENCODING_UTF8: u32 = 0x0800_0100;
        unsafe {
            let arr = CFLocaleCopyPreferredLanguages();
            if !arr.is_null() {
                let mut is_ja = false;
                if CFArrayGetCount(arr) > 0 {
                    let s = CFArrayGetValueAtIndex(arr, 0);
                    if !s.is_null() {
                        let mut buf = [0i8; 64];
                        if CFStringGetCString(s, buf.as_mut_ptr(), buf.len() as isize, K_CF_STRING_ENCODING_UTF8) != 0 {
                            if let Ok(cstr) = std::ffi::CStr::from_ptr(buf.as_ptr()).to_str() {
                                is_ja = cstr.to_ascii_lowercase().starts_with("ja");
                            }
                        }
                    }
                }
                CFRelease(arr);
                return is_ja;
            }
        }
        std::env::var("LANG")
            .or_else(|_| std::env::var("LC_ALL"))
            .map(|v| v.to_ascii_lowercase().starts_with("ja"))
            .unwrap_or(false)
    }
}

impl Lang {
    pub fn detect_os_default() -> Self {
        #[cfg(windows)]
        {
            let lang_id = unsafe { GetUserDefaultUILanguage() };
            // Primary language ID 0x11 == LANG_JAPANESE
            if (lang_id & 0x03FF) == 0x11 {
                Lang::Ja
            } else {
                Lang::En
            }
        }
        #[cfg(target_os = "macos")]
        {
            if macos_lang::is_japanese() {
                Lang::Ja
            } else {
                Lang::En
            }
        }
        #[cfg(not(any(windows, target_os = "macos")))]
        {
            let is_ja = std::env::var("LANG")
                .or_else(|_| std::env::var("LC_ALL"))
                .map(|v| v.to_ascii_lowercase().starts_with("ja"))
                .unwrap_or(false);
            if is_ja {
                Lang::Ja
            } else {
                Lang::En
            }
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
