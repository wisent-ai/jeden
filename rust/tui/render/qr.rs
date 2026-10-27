//! Terminal QR rendering for shareable links (collab relay URLs, device-code
//! verification URIs). Unicode half-block output, so it renders the same in
//! the TUI transcript, tmux captures, and plain terminals.

/// Render `data` as a compact unicode QR code, or the encoder's reason it
/// cannot (a payload too large for a QR symbol); callers then show the link
/// as text with that reason.
pub fn render(data: &str) -> Result<String, qrcode::types::QrError> {
    let code = qrcode::QrCode::new(data.as_bytes())?;
    Ok(code
        .render::<qrcode::render::unicode::Dense1x2>()
        .quiet_zone(true)
        .build())
}
