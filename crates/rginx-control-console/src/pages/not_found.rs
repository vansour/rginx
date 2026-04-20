use super::*;

#[component]
pub fn NotFound(segments: Vec<String>) -> Element {
    let missing_path = format!("/{}", segments.join("/"));
    rsx! {
        section { class: "page-shell page-shell--narrow",
            article { class: "panel panel--stack",
                header { class: "panel__header",
                    h2 { "页面不存在" }
                    span { "404" }
                }
                p { "无法匹配的路径: {missing_path}" }
                Link { class: "primary-button", to: Route::Dashboard {}, "返回控制台" }
            }
        }
    }
}
