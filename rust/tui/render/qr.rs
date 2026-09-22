//! Terminal QR rendering for shareable links (collab relay URLs, device-code
//! verification URIs). Unicode half-block output, so it renders the same in
//! the TUI transcript, tmux captures, and plain terminals.

/// Render `data` as a compact unicode QR code. Returns `None` when the
/// payload cannot be encoded (too large for a QR symbol); callers then fall
/// back to showing the raw text only.
pub fn render(data: &str) -> Option<String> {
    let code = qrcode::QrCode::new(data.as_bytes()).ok()?;
    Some(
        code.render::<qrcode::render::unicode::Dense1x2>()
            .quiet_zone(true)
            .build(),
    )
}

#[cfg(test)]
mod tests {
    #[test]
    fn renders_unicode_blocks_for_urls() {
        let qr = super::render("https://relay.example/room/abc#key=xyz&role=view")
            .expect("url fits a qr symbol");
        assert!(qr.contains('█') || qr.contains('▀') || qr.contains('▄'));
        assert!(qr.lines().count() > 5);
    }

    #[test]
    fn absurdly_long_payload_returns_none() {
        let payload = "x".repeat(5000);
        assert!(super::render(&payload).is_none());
    }
}
