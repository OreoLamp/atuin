use std::{env, net::TcpListener, time::Duration};

use atuin_client::api_client;
use atuin_common::{api::AddHistoryRequest, utils::uuid_v7};
use atuin_server::{launch_with_listener, Settings as ServerSettings};
use atuin_server_postgres::{Postgres, PostgresSettings};
use futures_util::TryFutureExt;
use time::OffsetDateTime;
use tokio::{sync::oneshot, task::JoinHandle};
use tracing::{dispatcher, Dispatch};
use tracing_subscriber::{layer::SubscriberExt, EnvFilter};

async fn start_server(path: &str) -> (String, oneshot::Sender<()>, JoinHandle<()>) {
    let formatting_layer: tracing_tree::HierarchicalLayer<tracing_subscriber::fmt::TestWriter> = tracing_tree::HierarchicalLayer::default()
        .with_writer(tracing_subscriber::fmt::TestWriter::new())
        .with_indent_lines(true)
        .with_ansi(true)
        .with_targets(true)
        .with_indent_amount(2);

    let dispatch: Dispatch = tracing_subscriber::registry()
        .with(formatting_layer)
        .with(EnvFilter::new("atuin_server=debug,atuin_client=debug,info"))
        .into();

    let db_uri: String = env::var("ATUIN_DB_URI")
        .unwrap_or_else(|_| "postgres://atuin:pass@localhost:5432/atuin".to_owned());

    let server_settings: ServerSettings<PostgresSettings> = ServerSettings {
        host: "127.0.0.1".to_owned(),
        port: 0,
        path: path.to_owned(),
        open_registration: true,
        max_history_length: 8192,
        max_record_size: 1024 * 1024 * 1024,
        page_size: 1100,
        register_webhook_url: None,
        register_webhook_username: String::new(),
        db_settings: PostgresSettings { db_uri },
        metrics: atuin_server::settings::Metrics::default(),
    };

    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
    let listener: TcpListener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr: std::net::SocketAddr = listener.local_addr().unwrap();
    let server: JoinHandle<()> = tokio::spawn(async move {
        let _tracing_guard: dispatcher::DefaultGuard = dispatcher::set_default(&dispatch);

        if let Err(e) = launch_with_listener::<Postgres>(
            server_settings,
            listener,
            shutdown_rx.unwrap_or_else(|_| ()),
        )
        .await
        {
            tracing::error!(error=?e, "server error");
            panic!("error running server: {e:?}");
        }
    });

    // let the server come online
    tokio::time::sleep(Duration::from_millis(200)).await;

    (format!("http://{addr}{path}"), shutdown_tx, server)
}

async fn register_inner<'a>(
    address: &'a str,
    username: &str,
    password: &str,
) -> api_client::Client<'a> {
    let email: String = format!("{}@example.com", uuid_v7().as_simple());

    // registration works
    let registration_response: atuin_common::api::RegisterResponse = api_client::register(address, username, &email, password)
        .await
        .unwrap();

    api_client::Client::new(address, &registration_response.session, 5, 30).unwrap()
}

async fn login(address: &str, username: String, password: String) -> api_client::Client<'_> {
    // registration works
    let login_respose: atuin_common::api::LoginResponse = api_client::login(
        address,
        atuin_common::api::LoginRequest { username, password },
    )
    .await
    .unwrap();

    api_client::Client::new(address, &login_respose.session, 5, 30).unwrap()
}

async fn register(address: &str) -> api_client::Client<'_> {
    let username: String = uuid_v7().as_simple().to_string();
    let password: String = uuid_v7().as_simple().to_string();
    register_inner(address, &username, &password).await
}

#[tokio::test]
async fn registration() {
    let path: String = format!("/{}", uuid_v7().as_simple());
    let (address, shutdown, server) = start_server(&path).await;
    dbg!(&address);

    // -- REGISTRATION --

    let username: String = uuid_v7().as_simple().to_string();
    let password: String = uuid_v7().as_simple().to_string();
    let client: api_client::Client<'_> = register_inner(&address, &username, &password).await;

    // the session token works
    let status: atuin_common::api::StatusResponse = client.status().await.unwrap();
    assert_eq!(status.username, username);

    // -- LOGIN --

    let client: api_client::Client<'_> = login(&address, username.clone(), password).await;

    // the session token works
    let status: atuin_common::api::StatusResponse = client.status().await.unwrap();
    assert_eq!(status.username, username);

    shutdown.send(()).unwrap();
    server.await.unwrap();
}

#[tokio::test]
async fn sync() {
    let path: String = format!("/{}", uuid_v7().as_simple());
    let (address, shutdown, server) = start_server(&path).await;

    let client: api_client::Client<'_> = register(&address).await;
    let hostname: String = uuid_v7().as_simple().to_string();
    let now: OffsetDateTime = OffsetDateTime::now_utc();

    let data1: String = uuid_v7().as_simple().to_string();
    let data2: String = uuid_v7().as_simple().to_string();

    client
        .post_history(&[
            AddHistoryRequest {
                id: uuid_v7().as_simple().to_string(),
                timestamp: now,
                data: data1.clone(),
                hostname: hostname.clone(),
            },
            AddHistoryRequest {
                id: uuid_v7().as_simple().to_string(),
                timestamp: now,
                data: data2.clone(),
                hostname: hostname.clone(),
            },
        ])
        .await
        .unwrap();

    let history: atuin_common::api::SyncHistoryResponse = client
        .get_history(OffsetDateTime::UNIX_EPOCH, OffsetDateTime::UNIX_EPOCH, None)
        .await
        .unwrap();

    assert_eq!(history.history, vec![data1, data2]);

    shutdown.send(()).unwrap();
    server.await.unwrap();
}
