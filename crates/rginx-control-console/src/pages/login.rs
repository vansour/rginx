use super::*;

#[component]
pub fn Login() -> Element {
    let navigator = use_navigator();
    let session = use_session();
    let actor = (session.actor)();
    let session_ready = (session.ready)();
    let login_pending = use_signal(|| false);
    let error = use_signal(|| None::<String>);
    let mut credentials =
        use_signal(|| LoginForm { username: String::new(), password: String::new() });

    let actor_snapshot = actor.clone();
    use_effect(use_reactive!(|(session_ready, actor_snapshot)| {
        if session_ready && actor_snapshot.is_some() {
            navigator.replace(Route::Dashboard {});
        }
    }));

    let handle_login = move |_| {
        let current = credentials();
        let request = AuthLoginRequest { username: current.username, password: current.password };
        to_owned![session, login_pending, error, navigator];
        spawn(async move {
            login_pending.set(true);
            error.set(None);
            match api::login(&request).await {
                Ok(response) => {
                    if let Err(store_error) = api::set_stored_auth_token(&response.token) {
                        error.set(Some(store_error.to_string()));
                    } else {
                        session.actor.set(Some(response.actor));
                        session.ready.set(true);
                        session.loading.set(false);
                        navigator.replace(Route::Dashboard {});
                    }
                }
                Err(login_error) => {
                    reset_session(session);
                    error.set(Some(login_error.to_string()));
                }
            }
            login_pending.set(false);
        });
    };

    rsx! {
        section { class: "page-shell page-shell--narrow",
            article { class: "panel auth-panel panel--stack",
                header { class: "panel__header",
                    h2 { "控制台登录" }
                    span { "本地账号认证" }
                }
                p { "先输入账号和密码，再进入总览与节点控制台。" }

                if !session_ready {
                    StateBanner { tone: "info", message: "正在同步本地会话…" }
                } else if let Some(message) = error() {
                    StateBanner { tone: "error", message }
                } else if actor.is_some() {
                    StateBanner { tone: "info", message: "已登录，正在进入控制台…" }
                }

                if actor.is_none() {
                    div { class: "field-grid",
                        label { class: "field",
                            span { "用户名" }
                            input {
                                value: credentials().username.clone(),
                                autocomplete: "username",
                                oninput: move |event| credentials.write().username = event.value(),
                            }
                        }
                        label { class: "field",
                            span { "密码" }
                            input {
                                r#type: "password",
                                value: credentials().password.clone(),
                                autocomplete: "current-password",
                                oninput: move |event| credentials.write().password = event.value(),
                            }
                        }
                    }
                    div { class: "auth-actions",
                        button {
                            class: "primary-button",
                            onclick: handle_login,
                            disabled: login_pending(),
                            if login_pending() { "正在登录…" } else { "登录控制台" }
                        }
                        p { class: "auth-hint", "使用已开通的控制台本地账号登录。" }
                    }
                }
            }
        }
    }
}
