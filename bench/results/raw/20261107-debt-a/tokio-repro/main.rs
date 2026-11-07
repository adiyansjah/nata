use tokio::{io::{AsyncReadExt, AsyncWriteExt}, net::{TcpListener, TcpStream}};
fn main() {
    tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all()
        .build().expect("runtime").block_on(async {
        for _ in 0..100 {
            let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
            let address = listener.local_addr().expect("address");
            let receiver = tokio::spawn(async move {
                let (mut socket, _) = listener.accept().await.expect("accept");
                assert_eq!(socket.read_u8().await.expect("read"), 42);
            });
            let mut sender = TcpStream::connect(address).await.expect("connect");
            sender.write_u8(42).await.expect("write");
            receiver.await.expect("join");
        }
    });
}
