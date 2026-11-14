//! Ask before submitting an unknown-origin torrent to Core or the stream server.
use slint::ComponentHandle;
use std::sync::{
    Mutex, OnceLock,
    atomic::{AtomicU64, Ordering},
};
use stremio_core::{models::player::Selected, types::resource::StreamSource};
use tokio::sync::oneshot;

struct Pending {
    token: String,
    hash: String,
    answer: oneshot::Sender<bool>,
}
static PENDING: OnceLock<Mutex<Option<Pending>>> = OnceLock::new();
static NEXT_TOKEN: AtomicU64 = AtomicU64::new(1);

fn pending() -> &'static Mutex<Option<Pending>> {
    PENDING.get_or_init(Default::default)
}

fn answer(token: &str, accepted: bool) {
    let request = {
        let mut current = pending().lock().unwrap_or_else(|e| e.into_inner());
        if current
            .as_ref()
            .is_none_or(|request| request.token != token)
        {
            return;
        }
        current.take()
    };
    if let Some(request) = request {
        if accepted {
            core_env::vpn_policy::confirm_foreign_source(&request.hash);
        }
        let _ = request.answer.send(accepted);
    }
}

pub fn install(ui: &crate::MainWindow) {
    let weak = ui.as_weak();
    ui.on_country_confirmation_answer(move |token, accepted| {
        answer(token.as_str(), accepted);
        if let Some(ui) = weak.upgrade()
            && ui.get_country_confirmation_token() == token
        {
            ui.set_country_confirmation_token("".into());
            ui.invoke_focus_app_shortcuts();
        }
    });
}

pub async fn ensure_confirmed(
    ui: slint::Weak<crate::MainWindow>,
    selected: &Selected,
    title: String,
) -> bool {
    let StreamSource::Torrent { info_hash, .. } = &selected.stream.source else {
        return true;
    };
    let hash = hex::encode(info_hash);
    crate::vpn::register_selection(selected);
    if !core_env::vpn_policy::requires_vpn(Some(&hash)) {
        return true;
    }
    let token = NEXT_TOKEN.fetch_add(1, Ordering::Relaxed).to_string();
    let (sender, receiver) = oneshot::channel();
    {
        // Replacing a request cancels the old choice, never approves it.
        let mut current = pending().lock().unwrap_or_else(|e| e.into_inner());
        *current = Some(Pending {
            token: token.clone(),
            hash,
            answer: sender,
        });
    }
    let queued_token = token.clone();
    if slint::invoke_from_event_loop(move || {
        let still_current = pending()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .is_some_and(|request| request.token == queued_token);
        if !still_current {
            return;
        }
        if let Some(ui) = ui.upgrade() {
            ui.set_country_confirmation_title(title.into());
            ui.set_country_confirmation_token(queued_token.into());
        } else {
            answer(&queued_token, false);
        }
    })
    .is_err()
    {
        answer(&token, false);
    }
    receiver.await.unwrap_or(false)
}
