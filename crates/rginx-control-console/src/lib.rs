mod api;
mod display;
mod pages;
mod runtime;

use dioxus::prelude::*;

use rginx_control_types::AuthenticatedActor;

pub(crate) use pages::{
    Dashboard, EdgeNodes, Login, NodeDetail, NodeTls, NotFound,
};

#[derive(Clone, Copy)]
pub(crate) struct SessionContext {
    pub actor: Signal<Option<AuthenticatedActor>>,
    pub loading: Signal<bool>,
    pub ready: Signal<bool>,
}

impl SessionContext {
    #[cfg(target_arch = "wasm32")]
    fn new() -> Self {
        Self { actor: Signal::new(None), loading: Signal::new(true), ready: Signal::new(false) }
    }
}

#[derive(Routable, Clone, Debug, PartialEq)]
#[rustfmt::skip]
pub enum Route {
    #[route("/login")]
    Login {},
    #[layout(ConsoleShell)]
        #[route("/")]
        Dashboard {},
        #[route("/nodes")]
        EdgeNodes {},
        #[route("/nodes/:node_id")]
        NodeDetail { node_id: String },
        #[route("/nodes/:node_id/tls")]
        NodeTls { node_id: String },
    #[end_layout]
    #[route("/:..segments")]
    NotFound { segments: Vec<String> },
}

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen(start)]
pub fn start() {
    dioxus::LaunchBuilder::web().with_cfg(dioxus::web::Config::new().rootname("app")).launch(app);
}

#[cfg(target_arch = "wasm32")]
fn app() -> Element {
    let mut session = use_context_provider(SessionContext::new);

    use_effect(move || {
        let token = api::stored_auth_token();
        if token.is_none() {
            session.actor.set(None);
            session.loading.set(false);
            session.ready.set(true);
            return;
        }

        to_owned![session];
        spawn(async move {
            session.loading.set(true);
            match api::get_me().await {
                Ok(actor) => session.actor.set(Some(actor)),
                Err(_) => {
                    api::clear_stored_auth_token();
                    session.actor.set(None);
                }
            }
            session.loading.set(false);
            session.ready.set(true);
        });
    });

    rsx! {
        Router::<Route> {}
    }
}

#[component]
fn ConsoleShell() -> Element {
    let navigator = use_navigator();
    let session = use_session();
    let current_route = use_route::<Route>();
    let actor = (session.actor)();
    let session_ready = (session.ready)();
    let actor_name = actor
        .as_ref()
        .map(|actor| actor.user.display_name.clone())
        .unwrap_or_else(|| "未登录".to_string());
    let actor_meta = if (session.loading)() {
        "正在同步会话".to_string()
    } else {
        actor
            .as_ref()
            .map(|actor| format!("{} · 管理员", actor.user.username))
            .unwrap_or_else(|| "请先登录控制台".to_string())
    };

    let current_title = route_title(&current_route);
    let avatar = actor_name.chars().next().unwrap_or('R');

    let actor_snapshot = actor.clone();
    use_effect(use_reactive!(|(session_ready, actor_snapshot)| {
        if session_ready && actor_snapshot.is_none() {
            navigator.replace(Route::Login {});
        }
    }));

    let signout = move |_| {
        to_owned![session, navigator];
        spawn(async move {
            let _ = api::logout().await;
            reset_session(session);
            navigator.replace(Route::Login {});
        });
    };

    if !session_ready {
        return rsx! {
            section { class: "page-shell page-shell--narrow",
                article { class: "panel auth-panel panel--stack",
                    header { class: "panel__header",
                        h2 { "正在同步会话" }
                        span { "请稍候" }
                    }
                    p { "正在确认本地登录状态…" }
                }
            }
        };
    }

    if actor.is_none() {
        return rsx! {
            section { class: "page-shell page-shell--narrow",
                article { class: "panel auth-panel panel--stack",
                    header { class: "panel__header",
                        h2 { "需要登录" }
                        span { "访问受限" }
                    }
                    p { "正在跳转到登录页…" }
                }
            }
        };
    }

    rsx! {
        div { class: "app-shell",
            aside { class: "app-sidebar",
                Link {
                    class: "app-brand",
                    to: Route::Dashboard {},
                    span { class: "app-brand__mark", "RG" }
                    span {
                        strong { "rginx Console" }
                    }
                }

                nav { class: "app-sidebar__nav",
                    SidebarLink {
                        to: Route::Dashboard {},
                        active: is_dashboard_route(&current_route),
                        marker: "总",
                        label: "总览"
                    }
                    SidebarLink {
                        to: Route::EdgeNodes {},
                        active: is_nodes_route(&current_route),
                        marker: "节",
                        label: "节点"
                    }
                }
            }

            div { class: "app-main",
                div { class: "app-main__header",
                    header { class: "app-topbar",
                        h1 { class: "app-topbar__title", "{current_title}" }
                        div { class: "app-topbar__actions",
                            div { class: "app-user-card",
                                span { class: "app-user-card__avatar", "{avatar}" }
                                div {
                                    strong { "{actor_name}" }
                                    small { "{actor_meta}" }
                                }
                            }
                            if actor.is_some() {
                                button {
                                    class: "secondary-button secondary-button--compact",
                                    onclick: signout,
                                    "退出登录"
                                }
                            }
                        }
                    }
                }

                main { class: "app-content",
                    Outlet::<Route> {}
                }
            }
        }
    }
}

#[component]
fn SidebarLink(
    to: Route,
    active: bool,
    marker: &'static str,
    label: &'static str,
) -> Element {
    rsx! {
        Link {
            to,
            class: if active { "app-nav-link app-nav-link--active" } else { "app-nav-link" },
            span { class: "app-nav-link__marker", "{marker}" }
            span { class: "app-nav-link__body",
                strong { "{label}" }
            }
        }
    }
}

pub(crate) fn use_session() -> SessionContext {
    use_context::<SessionContext>()
}

pub(crate) fn reset_session(mut session: SessionContext) {
    api::clear_stored_auth_token();
    session.actor.set(None);
    session.loading.set(false);
    session.ready.set(true);
}

fn is_dashboard_route(route: &Route) -> bool {
    matches!(route, Route::Dashboard {})
}

fn is_nodes_route(route: &Route) -> bool {
    matches!(route, Route::EdgeNodes {} | Route::NodeDetail { .. } | Route::NodeTls { .. })
}

fn route_title(route: &Route) -> &'static str {
    match route {
        Route::Login {} => "控制台登录",
        Route::Dashboard {} => "控制台总览",
        Route::EdgeNodes {} => "节点概览",
        Route::NodeDetail { .. } => "节点详情",
        Route::NodeTls { .. } => "节点 TLS",
        Route::NotFound { .. } => "页面不存在",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rginx_control_types::{AuthRole, AuthSessionSummary, AuthUserSummary};

    fn actor_with_roles(roles: Vec<AuthRole>) -> AuthenticatedActor {
        AuthenticatedActor {
            user: AuthUserSummary {
                user_id: "user-1".to_string(),
                username: "console-user".to_string(),
                display_name: "Console User".to_string(),
                active: true,
                roles,
                created_at_unix_ms: 0,
            },
            session: AuthSessionSummary {
                session_id: "sess-1".to_string(),
                issued_at_unix_ms: 0,
                expires_at_unix_ms: 0,
            },
        }
    }

    #[test]
    fn route_group_helpers_cover_stage_four_routes() {
        assert!(is_dashboard_route(&Route::Dashboard {}));
        assert!(!is_dashboard_route(&Route::EdgeNodes {}));

        assert!(is_nodes_route(&Route::EdgeNodes {}));
        assert!(is_nodes_route(&Route::NodeDetail { node_id: "n-1".to_string() }));
        assert!(is_nodes_route(&Route::NodeTls { node_id: "n-1".to_string() }));
    }

    #[test]
    fn route_titles_are_defined_for_current_routes() {
        let cases = [
            (Route::Login {}, "控制台登录"),
            (Route::Dashboard {}, "控制台总览"),
            (Route::EdgeNodes {}, "节点概览"),
            (Route::NodeTls { node_id: "n-1".to_string() }, "节点 TLS"),
        ];

        for (route, expected_title) in cases {
            assert_eq!(route_title(&route), expected_title);
        }
    }

    #[test]
    fn session_actor_fixture_keeps_single_admin_role() {
        assert_eq!(
            actor_with_roles(vec![AuthRole::SuperAdmin]).user.roles,
            vec![AuthRole::SuperAdmin]
        );
    }
}
