//! 完整帧的边界回归不加载 DLL；同一组固定字节同时在 x64 和 x86 运行。

use tigerrime_contracts::{
    Candidate, Composition, InputState, KeyOutcome, MAX_CANDIDATES, MAX_TEXT_BYTES,
    protocol::{Command, Message, Rejection, Reply, Request, Response, Route, VERSION},
};
use tigerrime_ipc::{HEADER_LEN, MAX_BODY_LEN, decode, decode_response, encode, frame_len};

fn request(command: Command) -> Request {
    Request {
        route: Route {
            generation: u64::MAX,
            request_id: 2,
            session_id: 3,
        },
        command,
    }
}

fn reply(value: Reply) -> Message {
    Message::Response(Response {
        route: request(Command::Clear).route,
        reply: value,
    })
}

fn raw(body: &[u8]) -> Vec<u8> {
    let mut frame = b"TRIM".to_vec();
    frame.extend_from_slice(&VERSION.to_le_bytes());
    frame.extend_from_slice(&[0, 0]);
    frame.extend_from_slice(&(body.len() as u32).to_le_bytes());
    frame.extend_from_slice(body);
    frame
}

#[test]
fn handshake_and_notifications_reject_zero_identity_and_cannot_be_input_replies() {
    for message in [
        Message::Welcome {
            host: 0,
            generation: 1,
        },
        Message::Welcome {
            host: 1,
            generation: 0,
        },
        Message::Notification {
            token: tigerrime_contracts::candidate::CandidateToken {
                host: 1,
                generation: 1,
                session: 1,
                focus: 1,
                revision: 0,
            },
            reset: true,
        },
    ] {
        assert!(encode(&message).is_err());
        assert!(decode(&raw(&serde_json::to_vec(&message).unwrap())).is_err());
    }
    let expected = request(Command::Clear);
    for message in [
        Message::Hello,
        Message::Stopped,
        Message::Welcome {
            host: 1,
            generation: 1,
        },
    ] {
        assert!(decode_response(&encode(&message).unwrap(), &expected).is_err());
    }
}

#[test]
fn fixed_wire_bytes_do_not_depend_on_pointer_width() {
    let body = br#"{"type":"request","value":{"route":{"generation":18446744073709551615,"request_id":2,"session_id":3},"command":{"type":"key","value":{"key":{"type":"character","value":"u"},"epoch":1}}}}"#;
    let expected = Message::Request(request(Command::Key {
        epoch: 1,
        key: tigerrime_contracts::Key::Character('u'),
    }));
    assert_eq!(encode(&expected).unwrap(), raw(body));
    assert_eq!(decode(&raw(body)).unwrap(), expected);
    // 新焦点字段不向后兼容，旧 v2 即使正文形状正确也不得通过。
    let mut old = raw(body);
    old[4..6].copy_from_slice(&2u16.to_le_bytes());
    assert!(decode(&old).is_err());
    let mut state = InputState {
        page_index: u32::MAX as usize,
        ..Default::default()
    };
    assert_eq!(
        decode(&encode(&reply(Reply::Opened(state.clone()))).unwrap()).unwrap(),
        reply(Reply::Opened(state.clone()))
    );
    // 超出 x86 范围的合法 JSON 在两个架构上都必须拒绝。
    state.page_index = 0;
    let oversized = serde_json::to_string(&reply(Reply::Opened(state)))
        .unwrap()
        .replace("\"page_index\":0", "\"page_index\":4294967296");
    assert!(decode(&raw(oversized.as_bytes())).is_err());
}

#[test]
fn requests_and_all_replies_round_trip_owned_unicode() {
    let state = InputState {
        composition: Composition {
            text: "𠀀😀中".into(),
            cursor: 8,
            selection: 4..11,
        },
        candidates: vec![Candidate {
            text: "𠀀".into(),
            comment: "[U+20000]".into(),
            label: "㊀".into(),
            ..Default::default()
        }],
        page_size: 9,
        highlighted: Some(0),
        composing: true,
        ..Default::default()
    };
    let outcome = KeyOutcome {
        handled: true,
        commit: Some("𠀀😀".into()),
        state: state.clone(),
    };
    let messages = [
        Message::Hello,
        Message::Stop,
        Message::Stopped,
        Message::Welcome {
            generation: 7,
            host: 8,
        },
        Message::Notification {
            token: tigerrime_contracts::candidate::CandidateToken {
                host: 8,
                generation: 7,
                session: 1,
                focus: 2,
                revision: 3,
            },
            reset: false,
        },
        Message::Request(request(Command::Open {
            options: Default::default(),
        })),
        Message::Request(request(Command::Key {
            epoch: 1,
            key: tigerrime_contracts::Key::Character('𠀀'),
        })),
        Message::Request(request(Command::Clear)),
        Message::Request(request(Command::Configure {
            options: Default::default(),
        })),
        Message::Request(request(Command::Select {
            epoch: 1,
            revision: 1,
            index: 0,
        })),
        Message::Request(request(Command::Close)),
        Message::Request(request(Command::Focus {
            epoch: 1,
            caret: Some(tigerrime_contracts::candidate::ScreenRect {
                left: -10,
                top: 20,
                right: -9,
                bottom: 40,
            }),
        })),
        Message::Request(request(Command::Focus {
            epoch: 2,
            caret: None,
        })),
        Message::Request(request(Command::TakeSelection { epoch: 1 })),
        reply(Reply::Opened(state.clone())),
        reply(Reply::Configured(state.clone())),
        reply(Reply::Focused(state.clone())),
        reply(Reply::Cleared(state)),
        reply(Reply::Key(outcome)),
        reply(Reply::Closed),
        reply(Reply::Rejected(Rejection::SessionLimit)),
    ];
    for message in messages {
        let decoded = {
            let frame = encode(&message).unwrap();
            decode(&frame).unwrap()
        };
        assert_eq!(decoded, message);
    }
}

#[test]
fn rejects_invalid_headers_truncation_trailing_frames_and_unbounded_lengths() {
    let frame = encode(&Message::Request(request(Command::Close))).unwrap();
    for length in 0..frame.len() {
        assert!(decode(&frame[..length]).is_err(), "截断位置 {length}");
    }
    for offset in [0, 4, 6] {
        let mut changed = frame.clone();
        changed[offset] ^= 1;
        assert!(decode(&changed).is_err());
    }
    let mut header: [u8; HEADER_LEN] = frame[..HEADER_LEN].try_into().unwrap();
    for length in [0, MAX_BODY_LEN as u32 + 1, u32::MAX] {
        header[8..12].copy_from_slice(&length.to_le_bytes());
        assert!(frame_len(&header).is_err());
        assert!(decode(&header).is_err());
    }
    header[8..12].copy_from_slice(&(MAX_BODY_LEN as u32).to_le_bytes());
    assert_eq!(frame_len(&header).unwrap(), HEADER_LEN + MAX_BODY_LEN);
    let mut joined = frame.clone();
    joined.extend_from_slice(&frame);
    assert!(decode(&joined).is_err());
}

#[test]
fn rejects_invalid_utf8_json_unknown_fields_numbers_and_zero_ids() {
    for body in [b"\xff".as_slice(), b"{", b"null", b"[]", b"{}", b"{} {}"] {
        assert!(decode(&raw(body)).is_err());
    }
    let original = serde_json::to_string(&Message::Request(request(Command::Key {
        epoch: 1,
        key: tigerrime_contracts::Key::Character('u'),
    })))
    .unwrap();
    for (from, to) in [
        ("\"value\":\"u\"", "\"value\":\"uu\""),
        ("\"value\":\"u\"", "\"value\":12"),
        ("\"request_id\":2", "\"request_id\":2.5"),
        ("\"request_id\":2", "\"request_id\":-1"),
        ("\"request_id\":2", "\"request_id\":0"),
        ("\"session_id\":3", "\"session_id\":0"),
        ("\"generation\":18446744073709551615", "\"generation\":0"),
        ("\"request_id\":2", "\"request_id\":2,\"request_id\":3"),
        ("\"value\":\"u\"", "\"value\":\"u\",\"extra\":true"),
        ("\"session_id\":3", "\"session_id\":3,\"extra\":true"),
        ("\"type\":\"key\"", "\"type\":\"unknown\""),
    ] {
        let changed = original.replace(from, to);
        assert_ne!(changed, original);
        assert!(decode(&raw(changed.as_bytes())).is_err(), "{to}");
    }
    for command in [
        Command::Focus {
            epoch: 0,
            caret: None,
        },
        Command::Focus {
            epoch: 1,
            caret: Some(tigerrime_contracts::candidate::ScreenRect {
                left: i32::MIN,
                top: 0,
                right: 0,
                bottom: 1,
            }),
        },
        Command::Focus {
            epoch: 1,
            caret: Some(tigerrime_contracts::candidate::ScreenRect {
                left: 1,
                top: 0,
                right: 0,
                bottom: 1,
            }),
        },
        Command::TakeSelection { epoch: 0 },
        Command::Key {
            epoch: 0,
            key: tigerrime_contracts::Key::Space,
        },
        Command::Key {
            epoch: 1,
            key: tigerrime_contracts::Key::Character('\0'),
        },
        Command::Select {
            epoch: 1,
            revision: 0,
            index: 0,
        },
        Command::Select {
            epoch: 1,
            revision: 1,
            index: 9,
        },
    ] {
        let message = Message::Request(request(command));
        assert!(encode(&message).is_err());
        assert!(decode(&raw(&serde_json::to_vec(&message).unwrap())).is_err());
    }
}

fn rejects_state(state: InputState) {
    let message = reply(Reply::Opened(state));
    // 同时覆盖发送前与接收后检查，不能依赖发送端总是可信。
    assert!(encode(&message).is_err());
    assert!(decode(&raw(&serde_json::to_vec(&message).unwrap())).is_err());
}

#[test]
fn rejects_invalid_composition_and_candidate_boundaries() {
    for (cursor, start, end) in [(1, 0, 0), (5, 0, 0), (0, 1, 4), (0, 4, 0)] {
        rejects_state(InputState {
            composition: Composition {
                text: "𠀀".into(),
                cursor,
                // 倒置区间是故意构造的非法输入，不作为迭代器使用。
                selection: start..end,
            },
            ..Default::default()
        });
    }
    rejects_state(InputState {
        page_size: MAX_CANDIDATES + 1,
        ..Default::default()
    });
    rejects_state(InputState {
        candidates: vec![Candidate::default()],
        highlighted: Some(0),
        ..Default::default()
    });
    rejects_state(InputState {
        candidates: vec![Candidate::default()],
        page_size: 9,
        ..Default::default()
    });
    rejects_state(InputState {
        candidates: vec![Candidate::default()],
        page_size: 9,
        highlighted: Some(1),
        ..Default::default()
    });
    rejects_state(InputState {
        highlighted: Some(0),
        ..Default::default()
    });
    rejects_state(InputState {
        notice: "a".repeat(MAX_TEXT_BYTES + 1),
        ..Default::default()
    });
}

#[test]
fn limits_encoded_bytes_even_when_each_field_is_within_bounds() {
    let at_limit = reply(Reply::Key(KeyOutcome {
        handled: true,
        commit: Some("a".repeat(MAX_TEXT_BYTES)),
        state: InputState::default(),
    }));
    assert!(encode(&at_limit).is_ok());
    let oversized = reply(Reply::Key(KeyOutcome {
        handled: true,
        commit: Some("a".repeat(MAX_TEXT_BYTES + 1)),
        state: InputState::default(),
    }));
    assert!(encode(&oversized).is_err());
    // 转义后六倍膨胀，验证限制的是线上字节数而不是 Rust 字符串长度之和。
    let state = InputState {
        candidates: vec![
            Candidate {
                text: "\0".repeat(MAX_TEXT_BYTES),
                ..Default::default()
            };
            3
        ],
        page_size: 3,
        highlighted: Some(0),
        ..Default::default()
    };
    assert!(encode(&reply(Reply::Opened(state))).is_err());
}

#[test]
fn only_the_expected_route_and_reply_type_can_be_delivered() {
    let request = request(Command::Key {
        epoch: 1,
        key: tigerrime_contracts::Key::Space,
    });
    let response = Response {
        route: request.route,
        reply: Reply::Key(KeyOutcome {
            handled: true,
            commit: Some("的".into()),
            state: InputState::default(),
        }),
    };
    let frame = encode(&Message::Response(response.clone())).unwrap();
    assert_eq!(decode_response(&frame, &request).unwrap(), response);
    for field in ["generation", "request", "session"] {
        let mut expected = request.clone();
        match field {
            "generation" => expected.route.generation -= 1,
            "request" => expected.route.request_id += 1,
            _ => expected.route.session_id += 1,
        }
        assert!(decode_response(&frame, &expected).is_err());
    }
    assert!(decode_response(&encode(&reply(Reply::Closed)).unwrap(), &request).is_err());
    assert!(
        decode_response(
            &encode(&Message::Request(request.clone())).unwrap(),
            &request
        )
        .is_err()
    );
}
