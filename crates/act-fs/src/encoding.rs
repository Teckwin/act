//! Encoding engine: BOM detection, strict UTF-8, configurable fallback
//! (default GBK) via `encoding_rs`. This is the cure for mojibake when agents
//! read/write files created with locale encodings.

use act_kernel::error::{ActError, ActResult};

#[derive(Debug, Clone)]
pub struct Decoded {
    pub text: String,
    /// e.g. "utf-8", "utf-8-bom", "utf-16le", "gbk"
    pub encoding: String,
    pub lossy: bool,
}

/// Decode bytes: BOM detection -> strict UTF-8 -> fallback encoding.
pub fn decode(bytes: &[u8], fallback: &str) -> Decoded {
    if bytes.len() >= 3 && bytes[0] == 0xEF && bytes[1] == 0xBB && bytes[2] == 0xBF {
        let text = String::from_utf8_lossy(&bytes[3..]).into_owned();
        let lossy = std::str::from_utf8(&bytes[3..]).is_err();
        return Decoded {
            text,
            encoding: "utf-8-bom".into(),
            lossy,
        };
    }
    if bytes.len() >= 2 && bytes[0] == 0xFF && bytes[1] == 0xFE {
        let units: Vec<u16> = bytes[2..]
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        let text = String::from_utf16_lossy(&units);
        let lossy = String::from_utf16(&units).is_err();
        return Decoded {
            text,
            encoding: "utf-16le".into(),
            lossy,
        };
    }
    if bytes.len() >= 2 && bytes[0] == 0xFE && bytes[1] == 0xFF {
        let units: Vec<u16> = bytes[2..]
            .chunks_exact(2)
            .map(|c| u16::from_be_bytes([c[0], c[1]]))
            .collect();
        let text = String::from_utf16_lossy(&units);
        let lossy = String::from_utf16(&units).is_err();
        return Decoded {
            text,
            encoding: "utf-16be".into(),
            lossy,
        };
    }
    if let Ok(text) = std::str::from_utf8(bytes) {
        return Decoded {
            text: text.to_string(),
            encoding: "utf-8".into(),
            lossy: false,
        };
    }
    let label = if fallback.is_empty() { "gbk" } else { fallback };
    let enc = encoding_rs::Encoding::for_label(label.as_bytes()).unwrap_or(encoding_rs::GBK);
    let (cow, used, had_errors) = enc.decode(bytes);
    Decoded {
        text: cow.into_owned(),
        encoding: used.name().to_lowercase(),
        lossy: had_errors,
    }
}

/// Encode text with the requested encoding (default UTF-8 without BOM).
pub fn encode(text: &str, encoding: &str) -> ActResult<Vec<u8>> {
    match encoding.to_lowercase().as_str() {
        "" | "utf-8" | "utf8" => Ok(text.as_bytes().to_vec()),
        "utf-8-bom" | "utf8-bom" => {
            let mut out = vec![0xEF, 0xBB, 0xBF];
            out.extend_from_slice(text.as_bytes());
            Ok(out)
        }
        "utf-16le" | "utf16le" | "utf-16" => {
            let mut out = vec![0xFF, 0xFE];
            out.extend(text.encode_utf16().flat_map(u16::to_le_bytes));
            Ok(out)
        }
        "utf-16be" | "utf16be" => {
            let mut out = vec![0xFE, 0xFF];
            out.extend(text.encode_utf16().flat_map(u16::to_be_bytes));
            Ok(out)
        }
        other => {
            let enc = encoding_rs::Encoding::for_label(other.as_bytes()).ok_or_else(|| {
                ActError::invalid_params("encoding", format!("unsupported encoding '{other}'"))
            })?;
            let (cow, _used, had_errors) = enc.encode(text);
            if had_errors {
                return Err(ActError::invalid_params(
                    "encoding",
                    format!("text cannot be represented in '{other}'"),
                ));
            }
            Ok(cow.into_owned())
        }
    }
}

/// Line-ending normalization: "auto" (dominant), "lf", "crlf", "none".
pub fn normalize_eol(text: &str, mode: &str) -> String {
    let normalized = text.replace("\r\n", "\n");
    match mode.to_lowercase().as_str() {
        "none" => text.to_string(),
        "lf" => normalized.replace('\n', "\n"),
        "crlf" => normalized.replace('\n', "\r\n"),
        _ => {
            // auto: keep the dominant original style
            let crlf = text.matches("\r\n").count();
            let bare_lf = text.matches('\n').count().saturating_sub(crlf);
            if crlf >= bare_lf && crlf > 0 {
                normalized.replace('\n', "\r\n")
            } else {
                normalized
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf8_strict() {
        let d = decode("hello 中文".as_bytes(), "gbk");
        assert_eq!(d.encoding, "utf-8");
        assert!(!d.lossy);
        assert_eq!(d.text, "hello 中文");
    }

    #[test]
    fn utf8_bom_detected() {
        let mut bytes = vec![0xEF, 0xBB, 0xBF];
        bytes.extend_from_slice("x".as_bytes());
        let d = decode(&bytes, "gbk");
        assert_eq!(d.encoding, "utf-8-bom");
        assert_eq!(d.text, "x");
    }

    #[test]
    fn utf16le_detected() {
        let text = "中文测试";
        let bytes: Vec<u8> = std::iter::once(0xFF)
            .chain(std::iter::once(0xFE))
            .chain(text.encode_utf16().flat_map(u16::to_le_bytes))
            .collect();
        let d = decode(&bytes, "gbk");
        assert_eq!(d.encoding, "utf-16le");
        assert_eq!(d.text, text);
    }

    #[test]
    fn gbk_fallback() {
        // "中文" in GBK
        let bytes = [0xD6, 0xD0, 0xCE, 0xC4];
        let d = decode(&bytes, "gbk");
        assert_eq!(d.encoding, "gbk");
        assert!(!d.lossy);
        assert_eq!(d.text, "中文");
    }

    #[test]
    fn gbk_lossy_flagged() {
        // Invalid GBK sequence.
        let bytes = [0xD6, 0xD0, 0xFF, 0xFF];
        let d = decode(&bytes, "gbk");
        assert!(d.lossy);
    }

    #[test]
    fn encode_roundtrip() {
        let bytes = encode("中文", "gbk").unwrap();
        assert_eq!(bytes, vec![0xD6, 0xD0, 0xCE, 0xC4]);
        let d = decode(&bytes, "gbk");
        assert_eq!(d.text, "中文");

        let bom = encode("a", "utf-8-bom").unwrap();
        assert_eq!(bom, vec![0xEF, 0xBB, 0xBF, b'a']);

        let le = encode("中", "utf-16le").unwrap();
        assert_eq!(&le[..2], &[0xFF, 0xFE]);

        assert!(encode("中文", "ascii").is_err());
    }

    #[test]
    fn eol_modes() {
        assert_eq!(normalize_eol("a\r\nb\nc", "lf"), "a\nb\nc");
        assert_eq!(normalize_eol("a\nb", "crlf"), "a\r\nb");
        assert_eq!(normalize_eol("a\r\nb", "auto"), "a\r\nb");
        assert_eq!(normalize_eol("a\nb\nc", "auto"), "a\nb\nc");
        assert_eq!(normalize_eol("a\r\nb", "none"), "a\r\nb");
    }
}
