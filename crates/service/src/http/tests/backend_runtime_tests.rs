use super::{
    enqueue_isolated, http_enqueue_wait_timeout, http_queue_size, http_stream_queue_size,
    http_stream_worker_count, http_worker_count, panic_payload_message, EnqueueError,
    HttpQueueKind, HTTP_QUEUE_MIN, HTTP_STREAM_QUEUE_MIN, HTTP_STREAM_WORKER_MIN, HTTP_WORKER_MIN,
};
use crossbeam_channel::bounded;
use std::time::Duration;

#[test]
fn worker_count_has_minimum_guard() {
    assert!(http_worker_count() >= HTTP_WORKER_MIN);
    assert!(http_stream_worker_count() >= HTTP_STREAM_WORKER_MIN);
}

#[test]
fn queue_size_has_minimum_guard() {
    assert!(http_queue_size(0) >= HTTP_QUEUE_MIN);
    assert!(http_stream_queue_size(0) >= HTTP_STREAM_QUEUE_MIN);
}

#[test]
fn panic_payload_message_formats_common_payloads() {
    let text = "boom";
    assert_eq!(panic_payload_message(&text), "boom");

    let owned = String::from("owned boom");
    assert_eq!(panic_payload_message(&owned), "owned boom");
}

#[test]
fn enqueue_wait_timeout_uses_short_positive_default() {
    assert!(http_enqueue_wait_timeout() >= Duration::from_millis(1));
}

#[test]
fn full_stream_queue_does_not_spill_into_normal_queue() {
    let (normal_tx, normal_rx) = bounded::<u8>(1);
    let (stream_tx, _stream_rx) = bounded::<u8>(1);
    stream_tx.send(1).expect("fill stream queue");

    let result = enqueue_isolated(
        2,
        HttpQueueKind::Stream,
        &normal_tx,
        &stream_tx,
        Duration::from_millis(1),
    );

    match result {
        Err(EnqueueError::Overloaded(value, HttpQueueKind::Stream)) => assert_eq!(value, 2),
        _ => panic!("unexpected enqueue result"),
    }
    assert!(normal_rx.is_empty(), "stream overload must not spill to normal queue");
}

#[test]
fn full_normal_queue_does_not_spill_into_stream_queue() {
    let (normal_tx, normal_rx) = bounded::<u8>(1);
    let (stream_tx, stream_rx) = bounded::<u8>(1);
    normal_tx.send(1).expect("fill normal queue");

    let result = enqueue_isolated(
        2,
        HttpQueueKind::Normal,
        &normal_tx,
        &stream_tx,
        Duration::from_millis(1),
    );

    match result {
        Err(EnqueueError::Overloaded(value, HttpQueueKind::Normal)) => assert_eq!(value, 2),
        _ => panic!("unexpected enqueue result"),
    }
    assert!(stream_rx.is_empty(), "normal overload must not spill to stream queue");
    assert_eq!(normal_rx.recv().expect("retain original normal item"), 1);
}
