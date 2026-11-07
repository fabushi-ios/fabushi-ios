use std::io::{Read,Write};
use std::net::TcpListener;
use std::sync::{Arc,Mutex,atomic::{AtomicUsize,Ordering}};
use std::thread;
use std::time::{Duration,Instant};
use mahayana_host_runtime::extensions::notify_bus::notify_bus_client::*;

#[test]
fn parser_topics_and_retry_policy_match_frozen_contract(){
 assert_eq!(parse_notify_frame("data: {\"kind\":\"connected\"}"),SandNotifyFrame::Connected);
 assert_eq!(parse_notify_frame("data: {\"kind\":\"notify\",\"topic\":\"automation-fires\"}"),SandNotifyFrame::Notify(SandNotifyTopic::AutomationFires));
 assert_eq!(parse_notify_frame("data: {\"kind\":\"notify\",\"topic\":\"bogus\"}"),SandNotifyFrame::Ignored);
 assert_eq!(parse_notify_frame("event: ping"),SandNotifyFrame::Ignored);
 let timing=NotifyBusTiming{healthy_connection_min_lifetime_ms:30_000,reconnect_initial_delay_ms:1_000,reconnect_max_delay_ms:60_000,stall_ms:35_000};
 assert_eq!(reconnect_delay_ms(1,timing),1_000);assert_eq!(reconnect_delay_ms(7,timing),60_000);
 assert_eq!(next_reconnect_attempt(Some(1_000),31_000,5,timing),0);
 assert_eq!(next_reconnect_attempt(Some(1_000),30_999,5,timing),6);
}

#[test]
fn real_http_sse_stream_sends_auth_and_dispatches_frames(){
 let listener=TcpListener::bind("127.0.0.1:0").unwrap();let addr=listener.local_addr().unwrap();
 let request=Arc::new(Mutex::new(String::new()));let request2=Arc::clone(&request);
 let server=thread::spawn(move||{
  let (mut stream,_)=listener.accept().unwrap();let mut buf=[0u8;4096];let n=stream.read(&mut buf).unwrap();*request2.lock().unwrap()=String::from_utf8_lossy(&buf[..n]).into_owned();
  let body=b"data: {\"kind\":\"connected\"}\n\ndata: {\"kind\":\"notify\",\"topic\":\"listener-events\"}\n\n";
  let head=format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",body.len());
  stream.write_all(head.as_bytes()).unwrap();stream.write_all(body).unwrap();stream.flush().unwrap();
 });
 let connected=Arc::new(AtomicUsize::new(0));let c2=Arc::clone(&connected);
 let topics=Arc::new(Mutex::new(Vec::new()));let t2=Arc::clone(&topics);
 let errors=Arc::new(Mutex::new(Vec::new()));let e2=Arc::clone(&errors);
 let backend=format!("http://{addr}/");
 let client=SandNotifyBusClient::new(NotifyBusClientDependencies{
  get_backend_url:Arc::new(move||Ok(backend.clone())),
  get_access_token:Arc::new(|_|Ok("secret-token".into())),
  on_connected:Arc::new(move||{c2.fetch_add(1,Ordering::SeqCst);}),
  on_notify:Arc::new(move|topic|t2.lock().unwrap().push(topic)),
  on_stream_error:Arc::new(move|error|e2.lock().unwrap().push(error.to_string())),
  now_ms:Arc::new(||1_000),
  timing:NotifyBusTiming{healthy_connection_min_lifetime_ms:30_000,reconnect_initial_delay_ms:5_000,reconnect_max_delay_ms:5_000,stall_ms:1_000},
 }).unwrap();
 client.start();let deadline=Instant::now()+Duration::from_secs(2);
 while Instant::now()<deadline&&(connected.load(Ordering::SeqCst)==0||topics.lock().unwrap().is_empty()){thread::sleep(Duration::from_millis(10))}
 client.stop();server.join().unwrap();
 assert!(connected.load(Ordering::SeqCst)>=1);assert_eq!(topics.lock().unwrap().as_slice(),&[SandNotifyTopic::ListenerEvents]);
 let request=request.lock().unwrap().to_ascii_lowercase();assert!(request.contains("authorization: bearer secret-token"));assert!(request.contains("accept: text/event-stream"));
 assert!(errors.lock().unwrap().is_empty());
}
