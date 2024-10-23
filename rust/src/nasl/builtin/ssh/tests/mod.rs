mod server;

use std::sync::Arc;
use std::time::Duration;

use russh::server::Config as ServerConfig;
use russh::server::Server as _;
use russh_keys::key::KeyPair;
use server::TestServer;

use crate::check_err_matches;
use crate::nasl::test_prelude::*;
use crate::nasl::NoOpLoader;
use crate::storage::DefaultDispatcher;

use once_cell::sync::Lazy;
use std::sync::Mutex;

static LOCK: Lazy<Mutex<()>> = Lazy::new(Mutex::default);

const PORT: u16 = 2223;

fn default_config() -> ServerConfig {
    ServerConfig {
        keys: vec![KeyPair::generate_ed25519().unwrap()],
        ..Default::default()
    }
}

#[tokio::test]
async fn ssh_connect() {
    run_test(
        |mut t| {
            t.ok(format!(r#"id = ssh_connect(port:{});"#, PORT), 9000);
            check_err_matches!(
                t,
                format!(r#"id = ssh_connect(port:{}, keytype: "foo");"#, PORT),
                FunctionErrorKind::WrongArgument(_)
            );
            // TODO make this error variant better
            check_err_matches!(
                t,
                format!(r#"id = ssh_connect(port:{}, keytype: "");"#, PORT),
                FunctionErrorKind::Dirty(_)
            );
        },
        default_config(),
    )
    .await
}

#[tokio::test]
async fn ssh_auth() {
    run_test(
        |mut t| {
            t.run(format!(
                r#"session_id = ssh_connect(port: {}, keytype: "ssh-rsa,ecdsa-sha2-nistp256");"#,
                PORT
            ));
            // t.run(r#"#prompt = ssh_login_interactive(session_id, login: "user");"#);
            // t.run(r#"#display(prompt);"#);
            // t.run(r#"#auth = ssh_login_interactive_pass(session_id, pass: "pass");"#);
            // t.run(r#"#a = ssh_set_login(session_id, login: "admin");"#);
            t.run(r#"auth = ssh_userauth(session_id, login: "user", password: "pass");"#);
            // t.run(r#"display(auth);"#);
        },
        default_config(),
    )
    .await
}

async fn run_test(
    f: impl Fn(TestBuilder<NoOpLoader, DefaultDispatcher>) -> () + Send + 'static,
    config: ServerConfig,
) {
    // Acquire the global lock to prevent multiple
    // tests from opening a server at the same time.
    let _guard = LOCK.lock();
    let server = tokio::spawn(run_server(config));
    let client = tokio::task::spawn_blocking(move || {
        std::thread::sleep(Duration::from_millis(200));
        let t = TestBuilder::default();
        f(t)
    });
    // Simply wait for whatever the test does on the client side
    let res = client.await;
    // and then abort the server, to make sure we do not run it for
    // all eternity.
    server.abort();
    res.unwrap()
}

async fn run_server(config: ServerConfig) {
    let config = Arc::new(config);
    let mut server = TestServer::default();
    server
        .run_on_address(config, ("0.0.0.0", PORT))
        .await
        .unwrap();
}
