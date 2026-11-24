use std::io::Write;
use flate2::Compression;
use flate2::write::GzEncoder;
use sha2::{Digest, Sha384};
use zerho_config::BudgetsConfig;
use zerho_core::{ZerhoError, ZerhoResult};
use zerho_transform::encode_base64;

use crate::Chunk;

/// Computes an RFC-compliant Subresource Integrity (`sha384-<base64>`) digest.
pub fn compute_sri_sha384(bytes: &[u8]) -> String {
    let digest = Sha384::digest(bytes);
    format!("sha384-{}", encode_base64(&digest))
}

/// Computes the real RFC 1952 Gzip compressed byte length for a chunk.
pub fn compute_gzip_size(bytes: &[u8]) -> usize {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    if encoder.write_all(bytes).is_err() {
        return bytes.len();
    }
    encoder.finish().map_or(bytes.len(), |buf| buf.len())
}

/// Compresses bytes using pure-Rust Brotli (quality 6, window 22) for `.br` static asset emission.
pub fn compress_brotli(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut writer = brotli::CompressorWriter::new(&mut out, 4096, 6, 22);
        if writer.write_all(bytes).is_err() {
            return bytes.to_vec();
        }
    }
    out
}

/// Scans an emitted client JS bundle for accidentally leaked credentials/private keys.
pub fn scan_for_leaked_secrets(file_name: &str, content: &str) -> ZerhoResult<()> {
    const PATTERNS: &[(&str, &str)] = &[
        ("AWS Access Key", "AKIA"),
        ("GitHub Personal Access Token", concat!("gh", "p_")),
        ("GitHub Fine-Grained PAT", concat!("github_", "pat_")),
        ("Stripe Live Secret Key", concat!("sk_", "live_")),
        ("Anthropic API Key", concat!("sk-", "ant-")),
        ("Private Key PEM Block", concat!("-----BEGIN ", "PRIVATE KEY-----")),
        ("RSA Private Key PEM Block", concat!("-----BEGIN ", "RSA PRIVATE KEY-----")),
    ];

    for &(label, prefix) in PATTERNS {
        if let Some(pos) = content.find(prefix) {
            if prefix == "AKIA" {
                let tail = &content[pos + 4..];
                let looks_like_aws = tail.len() >= 16
                    && tail[..16]
                        .bytes()
                        .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit());
                if !looks_like_aws {
                    continue;
                }
            }
            return Err(ZerhoError::Security(format!(
                "potential {label} detected in client bundle `{file_name}`! \
                 Remove secrets from frontend code or move them behind `[server.proxy]`."
            )));
        }
    }
    Ok(())
}

/// Enforces `[build.budgets]` against real gzipped chunk sizes.
pub fn enforce_budgets(chunks: &[Chunk], budgets: &BudgetsConfig) -> ZerhoResult<()> {
    let mut total_js_gzip: u64 = 0;
    let mut total_css_gzip: u64 = 0;

    for chunk in chunks {
        let gzip_u64 = u64::try_from(chunk.gzip_bytes).unwrap_or(u64::MAX);
        let is_js = std::path::Path::new(&chunk.file_name)
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("js"));
        let is_css = std::path::Path::new(&chunk.file_name)
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("css"));

        if is_js {
            total_js_gzip = total_js_gzip.saturating_add(gzip_u64);
        } else if is_css {
            total_css_gzip = total_css_gzip.saturating_add(gzip_u64);
        }

        if (is_js || is_css)
            && let Some(max_chunk_kb) = budgets.max_chunk_kb
            && gzip_u64 > max_chunk_kb * 1024
        {
            return Err(ZerhoError::Budget(format!(
                "chunk `{}` gzip size ({} B) exceeds `build.budgets.max_chunk_kb` ({max_chunk_kb} KiB)",
                chunk.file_name, chunk.gzip_bytes
            )));
        }
    }

    if let Some(max_js_kb) = budgets.max_js_kb
        && total_js_gzip > max_js_kb * 1024
    {
        return Err(ZerhoError::Budget(format!(
            "total JS gzip size ({total_js_gzip} B) exceeds `build.budgets.max_js_kb` ({max_js_kb} KiB)"
        )));
    }

    if let Some(max_css_kb) = budgets.max_css_kb
        && total_css_gzip > max_css_kb * 1024
    {
        return Err(ZerhoError::Budget(format!(
            "total CSS gzip size ({total_css_gzip} B) exceeds `build.budgets.max_css_kb` ({max_css_kb} KiB)"
        )));
    }

    Ok(())
}

/// Emits a strict `csp.json` manifest ready for reverse proxies / edge headers.
pub fn build_csp_manifest(chunks: &[Chunk]) -> String {
    let hashes: Vec<String> = chunks
        .iter()
        .filter_map(|c| c.integrity.as_ref())
        .map(|sri| format!("\"{sri}\""))
        .collect();
    format!(
        "{{\n  \"header\": \"Content-Security-Policy\",\n  \"policy\": \"default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self' data:; font-src 'self' https://fonts.gstatic.com; object-src 'none'; base-uri 'self'\",\n  \"sri_hashes\": [{}]\n}}\n",
        hashes.join(", ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use zerho_core::ContentHash;

    #[test]
    fn computes_standard_sha384_sri_and_gzip_size() {
        let sri = compute_sri_sha384(b"console.log('zerho');");
        assert!(sri.starts_with("sha384-"));
        assert_eq!(sri.len(), "sha384-".len() + 64);

        let repeated = "zerho-zero-node-rust-bundler;".repeat(200);
        let gz = compute_gzip_size(repeated.as_bytes());
        assert!(gz < repeated.len() / 5, "gzip should compress repetitive JS strongly");
    }

    #[test]
    fn detects_leaked_secrets_in_client_bundle() {
        assert!(scan_for_leaked_secrets("index.js", "export const ok = 42;").is_ok());
        let fake_stripe = format!("const k = \"{}1234567890abcdef\";", concat!("sk_", "live_"));
        assert!(scan_for_leaked_secrets("index.js", &fake_stripe).is_err());
        let fake_aws = format!("const a = \"{}IOSFODNN7EXAMPLE\";", "AKIA");
        assert!(scan_for_leaked_secrets("index.js", &fake_aws).is_err());
    }

    #[test]
    fn enforces_gzip_bundle_budgets() {
        let chunks = vec![Chunk {
            file_name: "assets/index-12345678.js".to_owned(),
            hash: ContentHash::from_bytes(b"js"),
            bytes: 120_000,
            gzip_bytes: 30_000,
            brotli_bytes: 24_000,
            brotli_file: None,
            sourcemap_file: None,
            integrity: None,
        }];
        let tight = BudgetsConfig {
            max_js_kb: Some(20),
            max_css_kb: None,
            max_chunk_kb: None,
        };
        assert!(enforce_budgets(&chunks, &tight).is_err());

        let generous = BudgetsConfig {
            max_js_kb: Some(50),
            max_css_kb: Some(10),
            max_chunk_kb: Some(40),
        };
        assert!(enforce_budgets(&chunks, &generous).is_ok());
    }
}
