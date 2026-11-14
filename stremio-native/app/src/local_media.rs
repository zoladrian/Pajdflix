/// Signs one local stream-server URL for mpv or another HTTP reader.
/// Remote, file and magnet inputs are returned unchanged.
pub(crate) fn media_url_for_player(url: String) -> String {
    match stream_server::security::authorize_local_media_url(&url) {
        Ok(signed) => signed,
        Err(error) => {
            tracing::debug!(%error, "playback URL left unchanged");
            url
        }
    }
}
