use super::*;

#[test]
fn check_supports_relative_includes_and_environment_expansion() {
    let temp_dir = temp_dir("rginx-check-include-env-test");
    fs::create_dir_all(temp_dir.join("fragments")).expect("temp fragments dir should be created");
    let config_path = temp_dir.join("rginx.ron");
    let routes_path = temp_dir.join("fragments/routes.ron");
    let listen_addr: SocketAddr = "127.0.0.1:18082".parse().unwrap();

    fs::write(
        &routes_path,
        "LocationConfig(\n    matcher: Exact(\"/\"),\n    handler: Return(\n        status: 200,\n        location: \"\",\n        body: Some(\"${rginx_check_body:-included body\\n}\"),\n    ),\n),\n",
    )
    .expect("routes fragment should be written");
    fs::write(
        &config_path,
        "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: \"${rginx_check_listen}\",\n    ),\n    upstreams: [],\n    locations: [\n        // @include \"fragments/routes.ron\"\n    ],\n)\n",
    )
    .expect("root config should be written");

    let output = Command::new(binary_path())
        .env("rginx_check_listen", listen_addr.to_string())
        .env("rginx_check_body", "body from env\n")
        .arg("check")
        .arg("--config")
        .arg(&config_path)
        .output()
        .expect("rginx should run");

    assert!(output.status.success(), "check should succeed: {}", render_output(&output));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains(&listen_addr.to_string()));
    assert!(
        stdout.contains("routes=1"),
        "check output should include the fragment route: {stdout}"
    );

    let _ = fs::remove_dir_all(temp_dir);
}
