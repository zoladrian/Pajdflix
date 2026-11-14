//! Local HTTP is an untrusted boundary, even when bound to loopback.
use axum::{
    extract::{Request, State},
    http::{Method, StatusCode},
    middleware::Next,
    response::Response,
};
use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};
use url::Url;

const LIFETIME: u64 = 12 * 60 * 60;
#[derive(Clone)]
pub struct LocalAuth {
    key: [u8; 32],
    capabilities: Arc<Mutex<HashMap<String, (u64, String)>>>,
}
#[derive(Clone)]
struct TrustedCall;

impl Default for LocalAuth {
    fn default() -> Self {
        let mut key = [0; 32];
        getrandom::fill(&mut key).expect("OS randomness is required for local API authentication");
        Self {
            key,
            capabilities: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before epoch")
        .as_secs()
}
fn media_path(path: &str) -> bool {
    let parts: Vec<_> = path.trim_start_matches('/').split('/').collect();
    let hash = |s: &str| s.len() == 40 && s.bytes().all(|b| b.is_ascii_hexdigit());
    let index = |s: &str| s == "-1" || (!s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()));
    match parts.as_slice() {
        ["proxy", tail @ ..] if !tail.is_empty() => true,
        ["subtitles.vtt" | "subtitles.srt" | "subtitles.ass" | "subtitles.ssa"] => true,
        ["stream", h, i] | [h, i] if hash(h) && index(i) => true,
        [h, i, "subtitles.vtt"] if hash(h) && index(i) => true,
        [h, i, tail @ ..] if hash(h) && index(i) && !tail.is_empty() && tail.len() <= 2 => {
            safe_playlist_resource(tail)
        }
        ["hlsv2", tail @ ..] if tail.len() >= 2 && tail.len() <= 4 => {
            tail.iter().all(|s| {
                !s.is_empty()
                    && *s != "."
                    && *s != ".."
                    && s.bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
            }) && tail.last().is_some_and(|s| {
                [".m3u8", ".m4s", ".ts", ".mp4", ".vtt"]
                    .iter()
                    .any(|ext| s.ends_with(ext))
            })
        }
        _ => false,
    }
}

fn safe_playlist_resource(tail: &[&str]) -> bool {
    tail.iter().all(|s| {
        !s.is_empty()
            && *s != "."
            && *s != ".."
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
    }) && tail.last().is_some_and(|s| {
        [".m3u8", ".m4s", ".ts", ".mp4", ".vtt"]
            .iter()
            .any(|ext| s.ends_with(ext))
    })
}

fn unsigned_resource(url: &Url) -> String {
    let mut query = url::form_urlencoded::Serializer::new(String::new());
    for (k, v) in url
        .query_pairs()
        .filter(|(k, _)| k != "_secure_cap" && k != "_secure_exp")
    {
        query.append_pair(&k, &v);
    }
    let query = query.finish();
    if query.is_empty() {
        url.path().to_owned()
    } else {
        format!("{}?{}", url.path(), query)
    }
}

impl LocalAuth {
    fn mac(&self, message: &[u8]) -> Hmac<Sha256> {
        let mut mac = Hmac::<Sha256>::new_from_slice(&self.key).expect("fixed key length");
        mac.update(message);
        mac
    }
    /// Explicit opt-in for a native HTTP connector. Never print or persist this value.
    pub fn api_bearer(&self) -> String {
        hex::encode(self.mac(b"local-api-v1").finalize().into_bytes())
    }
    fn verify_api(&self, token: &str) -> bool {
        hex::decode(token).is_ok_and(|bytes| self.mac(b"local-api-v1").verify_slice(&bytes).is_ok())
    }
    /// Capabilities authorize one read-only resource, including its query, for this session.
    pub fn authorize_media(&self, raw: &str, base: &str) -> anyhow::Result<String> {
        let mut url = Url::parse(raw)?;
        let base = Url::parse(base)?;
        anyhow::ensure!(
            url.origin() == base.origin() && url.username().is_empty() && url.password().is_none(),
            "media URL is not this local server"
        );
        anyhow::ensure!(
            media_path(url.path()),
            "route is not eligible for media access"
        );
        let resource = unsigned_resource(&url);
        let cache_key = format!("{}{}", base.origin().ascii_serialization(), resource);
        let mut cache = self
            .capabilities
            .lock()
            .map_err(|_| anyhow::anyhow!("media authorization lock poisoned"))?;
        if let Some((expiry, signed)) = cache
            .get(&cache_key)
            .filter(|(expiry, _)| *expiry > now() + 300)
        {
            let _ = expiry;
            return Ok(signed.clone());
        }
        let expiry = now() + LIFETIME;
        let signature = hex::encode(
            self.mac(format!("media-v1\n{expiry}\n{resource}").as_bytes())
                .finalize()
                .into_bytes(),
        );
        let clean: Vec<_> = url
            .query_pairs()
            .filter(|(k, _)| k != "_secure_cap" && k != "_secure_exp")
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect();
        url.set_query(None);
        url.query_pairs_mut()
            .extend_pairs(clean)
            .append_pair("_secure_exp", &expiry.to_string())
            .append_pair("_secure_cap", &signature);
        url.set_fragment(None);
        let signed: String = url.into();
        if cache.len() >= 4096 {
            cache.clear();
        }
        cache.insert(cache_key, (expiry, signed.clone()));
        Ok(signed)
    }
    fn verify_media(&self, url: &Url) -> bool {
        if !media_path(url.path()) {
            return false;
        }
        let caps: Vec<_> = url
            .query_pairs()
            .filter(|(k, _)| k == "_secure_cap")
            .map(|(_, v)| v.into_owned())
            .collect();
        let exps: Vec<_> = url
            .query_pairs()
            .filter(|(k, _)| k == "_secure_exp")
            .map(|(_, v)| v.into_owned())
            .collect();
        if caps.len() != 1 || exps.len() != 1 {
            return false;
        }
        let Ok(expiry) = exps[0].parse::<u64>() else {
            return false;
        };
        let time = now();
        if expiry < time || expiry > time.saturating_add(LIFETIME) {
            return false;
        }
        let Ok(signature) = hex::decode(&caps[0]) else {
            return false;
        };
        self.mac(format!("media-v1\n{expiry}\n{}", unsigned_resource(url)).as_bytes())
            .verify_slice(&signature)
            .is_ok()
    }
}

pub(crate) async fn trusted(mut request: Request, next: Next) -> Response {
    request.extensions_mut().insert(TrustedCall);
    next.run(request).await
}

pub(crate) async fn protect(
    State(state): State<crate::AppState>,
    mut request: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    let first = request.uri().path().split('/').nth(1).unwrap_or("");
    if ["rar", "zip", "7zip", "tar", "tgz", "nzb", "update"].contains(&first) {
        // Do not unpack unknown remote content or install upstream executables.
        return Err(StatusCode::FORBIDDEN);
    }
    if request.extensions().get::<TrustedCall>().is_none() {
        let base = Url::parse(&state.base_url).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        let expected_host = base[url::Position::BeforeHost..url::Position::AfterPort].to_owned();
        if request.headers().get("host").and_then(|v| v.to_str().ok())
            != Some(expected_host.as_str())
        {
            return Err(StatusCode::FORBIDDEN);
        }
        if let Some(origin) = request.headers().get("origin") {
            if origin.to_str().ok() != Some(base.origin().ascii_serialization().as_str()) {
                return Err(StatusCode::FORBIDDEN);
            }
        }
        let bearer = request
            .headers()
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.strip_prefix("Bearer "));
        let valid_api = bearer.is_some_and(|token| state.local_auth.verify_api(token));
        let valid_media = matches!(*request.method(), Method::GET | Method::HEAD)
            && base.join(&request.uri().to_string()).is_ok_and(|url| {
                url.origin() == base.origin() && state.local_auth.verify_media(&url)
            });
        if !valid_api && !valid_media {
            return Err(StatusCode::UNAUTHORIZED);
        }
    }
    let resource_url = Url::parse(&state.base_url)
        .ok()
        .and_then(|base| base.join(&request.uri().to_string()).ok());
    if let Some(url) = &resource_url {
        // Credentials must not reach proxy destinations, FFmpeg arguments or logs.
        if url
            .query_pairs()
            .any(|(key, _)| key == "_secure_cap" || key == "_secure_exp")
        {
            *request.uri_mut() = unsigned_resource(url)
                .parse()
                .map_err(|_| StatusCode::BAD_REQUEST)?;
        }
    }
    let mut response = next.run(request).await;
    if (resource_url
        .as_ref()
        .is_some_and(|url| url.path().ends_with(".m3u8"))
        || response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|s| s.contains("mpegurl")))
        && response.status().is_success()
    {
        let (mut parts, body) = response.into_parts();
        let bytes = axum::body::to_bytes(body, 1024 * 1024)
            .await
            .map_err(|_| StatusCode::BAD_GATEWAY)?;
        let playlist = std::str::from_utf8(&bytes).map_err(|_| StatusCode::BAD_GATEWAY)?;
        let rewritten = rewrite_playlist(
            &state.local_auth,
            playlist,
            resource_url.as_ref().unwrap(),
            &state.base_url,
        )?;
        parts.headers.remove("content-length");
        response = Response::from_parts(parts, axum::body::Body::from(rewritten));
    }
    // Do not allow a proxy target or caller-supplied response header to re-enable CORS.
    for name in [
        "access-control-allow-origin",
        "access-control-allow-credentials",
        "access-control-allow-headers",
        "access-control-allow-methods",
    ] {
        response.headers_mut().remove(name);
    }
    response
        .headers_mut()
        .insert("referrer-policy", "no-referrer".parse().unwrap());
    response
        .headers_mut()
        .insert("cache-control", "no-store".parse().unwrap());
    Ok(response)
}

fn rewrite_playlist(
    auth: &LocalAuth,
    body: &str,
    source: &Url,
    base: &str,
) -> Result<String, StatusCode> {
    let sign = |reference: &str| -> Result<String, StatusCode> {
        let url = source
            .join(reference)
            .map_err(|_| StatusCode::BAD_GATEWAY)?;
        if url.origin() != source.origin() {
            return Ok(reference.to_owned());
        }
        auth.authorize_media(url.as_str(), base)
            .map_err(|_| StatusCode::BAD_GATEWAY)
    };
    let mut output = String::new();
    for line in body.lines() {
        if !line.is_empty() && !line.starts_with('#') {
            output.push_str(&sign(line)?);
        } else if let Some((prefix, rest)) = line.split_once("URI=\"") {
            let (reference, suffix) = rest.split_once('"').ok_or(StatusCode::BAD_GATEWAY)?;
            output.push_str(prefix);
            output.push_str("URI=\"");
            output.push_str(&sign(reference)?);
            output.push('"');
            output.push_str(suffix);
        } else {
            output.push_str(line);
        }
        output.push('\n');
    }
    Ok(output)
}

/// Issue a scoped URL for native mpv/ffprobe; never put an API bearer in a URL.
pub fn authorize_local_media_url(raw: &str) -> anyhow::Result<String> {
    let url = Url::parse(raw)?;
    let state = crate::GLOBAL_STATE
        .read()
        .map_err(|_| anyhow::anyhow!("server state poisoned"))?;
    let Some(state) = state.as_ref() else {
        anyhow::ensure!(
            !url.host_str().is_some_and(|host| host == "localhost"
                || host
                    .parse::<std::net::IpAddr>()
                    .is_ok_and(|ip| ip.is_loopback())),
            "local server is not running"
        );
        return Ok(raw.to_owned());
    };
    if url.origin() != Url::parse(&state.base_url)?.origin() {
        return Ok(raw.to_owned());
    }
    state.local_auth.authorize_media(raw, &state.base_url)
}

/// The marker is a private Rust type. An HTTP header cannot impersonate it.
pub fn build_in_process_router(state: crate::AppState) -> axum::Router {
    crate::build_router(state).layer(axum::middleware::from_fn(trusted))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn capabilities_are_scoped_and_session_specific() {
        let auth = LocalAuth::default();
        let base = "http://127.0.0.1:11470";
        let raw = format!("{base}/stream/{}/0?track=2", "a".repeat(40));
        let signed = auth.authorize_media(&raw, base).unwrap();
        let url = Url::parse(&signed).unwrap();
        assert!(auth.verify_media(&url));
        assert!(!LocalAuth::default().verify_media(&url));
        let changed = signed.replace("track=2", "track=3");
        assert!(!auth.verify_media(&Url::parse(&changed).unwrap()));
        let mut changed = url.clone();
        changed.set_path("/removeAll");
        assert!(!auth.verify_media(&changed));
        let mut duplicate = url.clone();
        duplicate.query_pairs_mut().append_pair("_secure_exp", "1");
        assert!(!auth.verify_media(&duplicate));
        assert!(
            !auth.verify_api(
                url.query_pairs()
                    .find(|(k, _)| k == "_secure_cap")
                    .unwrap()
                    .1
                    .as_ref()
            )
        );
        assert!(auth.verify_api(&auth.api_bearer()));
        assert!(!auth.verify_api("bad"));
    }
    #[test]
    fn cannot_authorize_remote_or_mutating_urls() {
        let auth = LocalAuth::default();
        let base = "http://127.0.0.1:11470";
        for raw in [
            "http://example.com/stream/test/0",
            "http://127.0.0.1:11470/removeAll",
            "http://127.0.0.1:11470/hlsv2/id/destroy",
            "http://user@127.0.0.1:11470/settings",
        ] {
            assert!(auth.authorize_media(raw, base).is_err());
        }
    }
    #[test]
    fn playlist_links_receive_individual_capabilities() {
        let auth = LocalAuth::default();
        let base = "http://127.0.0.1:11470";
        let source = Url::parse(&format!("{base}/hlsv2/id/master.m3u8")).unwrap();
        let playlist = rewrite_playlist(
            &auth,
            "#EXTM3U\n#EXT-X-MAP:URI=\"init.mp4\"\n0.m4s\n",
            &source,
            base,
        )
        .unwrap();
        for raw in [
            playlist.lines().nth(1).unwrap().split('"').nth(1).unwrap(),
            playlist.lines().nth(2).unwrap(),
        ] {
            assert!(auth.verify_media(&Url::parse(raw).unwrap()));
        }
        assert!(rewrite_playlist(&auth, "../id/destroy\n", &source, base).is_err());
    }
    #[tokio::test]
    async fn local_http_rejects_unauthorized_calls_and_browser_origins() {
        use std::sync::Arc;
        use tower::ServiceExt;
        let dir = tempfile::tempdir().unwrap();
        let engine = Arc::new(
            enginefs::EngineFS::new(dir.path().join("cache"), Default::default())
                .await
                .unwrap(),
        );
        let state = crate::AppState::new(engine.clone(), Default::default(), dir.path().to_owned());
        let router = axum::Router::new()
            .route("/settings", axum::routing::get(|| async { "ok" }))
            .route(
                "/stream/{hash}/{index}",
                axum::routing::get(|uri: axum::http::Uri| async move { uri.to_string() })
                    .post(|| async { "must not run" }),
            )
            .layer(axum::middleware::from_fn_with_state(state.clone(), protect));
        for (bearer, host, origin, status) in [
            (None, "127.0.0.1:11470", None, StatusCode::UNAUTHORIZED),
            (
                Some("bad".to_owned()),
                "127.0.0.1:11470",
                None,
                StatusCode::UNAUTHORIZED,
            ),
            (
                Some(state.local_auth.api_bearer()),
                "attacker.test:11470",
                None,
                StatusCode::FORBIDDEN,
            ),
            (
                Some(state.local_auth.api_bearer()),
                "127.0.0.1:11470",
                Some("https://attacker.test"),
                StatusCode::FORBIDDEN,
            ),
            (
                Some(state.local_auth.api_bearer()),
                "127.0.0.1:11470",
                None,
                StatusCode::OK,
            ),
        ] {
            let mut request = axum::http::Request::builder()
                .uri("/settings")
                .header("host", host);
            if let Some(token) = bearer {
                request = request.header("authorization", format!("Bearer {token}"));
            }
            if let Some(origin) = origin {
                request = request.header("origin", origin);
            }
            let response = router
                .clone()
                .oneshot(request.body(axum::body::Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), status);
            assert!(
                response
                    .headers()
                    .get("access-control-allow-origin")
                    .is_none()
            );
        }
        // An attacker cannot forge the private Rust extension with a header.
        let raw = format!("{}/stream/{}/0?track=2", state.base_url, "a".repeat(40));
        let signed = state
            .local_auth
            .authorize_media(&raw, &state.base_url)
            .unwrap();
        let signed = Url::parse(&signed).unwrap();
        let uri = &signed[url::Position::BeforePath..];
        let request = axum::http::Request::builder()
            .uri(uri)
            .header("host", "127.0.0.1:11470")
            .body(axum::body::Body::empty())
            .unwrap();
        let response = router.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap();
        let body = std::str::from_utf8(&body).unwrap();
        assert!(body.ends_with("?track=2"));
        assert!(
            !body.contains("_secure_"),
            "media credentials reached the downstream handler"
        );
        let request = axum::http::Request::builder()
            .method("POST")
            .uri(uri)
            .header("host", "127.0.0.1:11470")
            .body(axum::body::Body::empty())
            .unwrap();
        assert_eq!(
            router.clone().oneshot(request).await.unwrap().status(),
            StatusCode::UNAUTHORIZED
        );
        let forged = axum::http::Request::builder()
            .uri("/settings")
            .header("host", "127.0.0.1:11470")
            .header("TrustedCall", "true")
            .body(axum::body::Body::empty())
            .unwrap();
        assert_eq!(
            router.clone().oneshot(forged).await.unwrap().status(),
            StatusCode::UNAUTHORIZED
        );
        let trusted = router.layer(axum::middleware::from_fn(super::trusted));
        let request = axum::http::Request::builder()
            .uri("/settings")
            .body(axum::body::Body::empty())
            .unwrap();
        assert_eq!(
            trusted.oneshot(request).await.unwrap().status(),
            StatusCode::OK
        );
        engine.stop().await;
    }

    #[test]
    fn real_loopback_api_and_shutdown() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = crate::ServerConfig::embedded();
        config.http_addr = "127.0.0.1:0".parse().unwrap();
        config.config_dir = Some(dir.path().join("config"));
        config.cache_dir = Some(dir.path().join("cache"));
        config.enable_cache_cleaner = false;
        let handle = crate::start(config).unwrap();
        let state = crate::GLOBAL_STATE.read().unwrap().clone().unwrap();
        let cancellation = state
            .engine
            .get_backend()
            .session
            .cancellation_token()
            .clone();
        let client = reqwest::blocking::Client::builder()
            .no_proxy()
            .timeout(std::time::Duration::from_secs(5))
            .build()
            .unwrap();
        let base = format!("http://{}", handle.http_addr());
        for path in [
            "/heartbeat",
            "/settings",
            "/diagnostics/export",
            "/removeAll",
            "/create",
        ] {
            assert_eq!(
                client.get(format!("{base}{path}")).send().unwrap().status(),
                StatusCode::UNAUTHORIZED,
                "{path}"
            );
        }
        let response = client
            .get(format!("{base}/heartbeat"))
            .bearer_auth(state.local_auth.api_bearer())
            .send()
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert!(
            response
                .headers()
                .get("access-control-allow-origin")
                .is_none()
        );
        assert_eq!(
            client
                .get(format!("{base}/heartbeat"))
                .bearer_auth(state.local_auth.api_bearer())
                .header("origin", "https://attacker.test")
                .send()
                .unwrap()
                .status(),
            StatusCode::FORBIDDEN
        );
        let address = handle.http_addr();
        handle.shutdown().unwrap();
        assert_eq!(
            handle.join().unwrap(),
            Some(crate::ShutdownSource::External)
        );
        assert!(
            cancellation.is_cancelled(),
            "torrent session survived server shutdown"
        );
        assert!(
            std::net::TcpStream::connect_timeout(&address, std::time::Duration::from_millis(200))
                .is_err()
        );
        assert!(crate::GLOBAL_STATE.read().unwrap().is_none());
    }
}
