use dezap::config::TlsConfig;
use dezap::net;

#[tokio::test]
#[ignore = "requires permission to bind UDP sockets"]
async fn client_and_server_connect() {
    let tls = TlsConfig::default();
    let net::ServerContext {
        endpoint,
        client_config: _,
    } = net::bind_server("127.0.0.1:0".parse().unwrap(), &tls).unwrap();
    let server_addr = endpoint.local_addr().unwrap();
    let server_endpoint = endpoint.clone();

    let client_ctx = net::build_client_endpoint("127.0.0.1:0".parse().unwrap(), &tls).unwrap();

    let client = net::connect(
        &client_ctx.endpoint,
        &client_ctx.client_config,
        server_addr,
        tls.server_name(),
    );
    let server = async move { server_endpoint.accept().await.unwrap().await.unwrap() };

    let (client_conn, server_conn) = tokio::join!(client, server);
    let _client_conn = client_conn.expect("client connection succeeds");
    let server_conn = server_conn;
    assert_eq!(
        server_conn.remote_address(),
        client_ctx.endpoint.local_addr().unwrap()
    );

    drop(client_ctx);
}
