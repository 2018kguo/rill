# What changed in HTTP/3

HTTP/3 maps HTTP semantics onto QUIC instead of TCP [1]. QUIC runs over UDP and
builds in TLS 1.3, so a connection needs one round trip before data flows, and a
resumed one can send data in its first flight ([0-RTT][rtt]).

## Why it matters

- **Head-of-line blocking** goes away across streams: a lost packet stalls only
  the stream it belongs to [2].
- Connection migration lets a phone switch from Wi-Fi to cellular without a new
  handshake, using connection IDs instead of the address four-tuple [1].
- Header compression moves from HPACK to QPACK, which tolerates out-of-order
  delivery [3].

| Layer      | HTTP/2      | HTTP/3          |
| ---------- | ----------- | --------------- |
| Transport  | TCP         | QUIC over UDP   |
| Encryption | TLS (opt.)  | TLS 1.3, always |
| Headers    | HPACK       | QPACK           |

```bash
curl --http3 -I https://cloudflare-quic.com
```

[1]: https://www.rfc-editor.org/rfc/rfc9114 "RFC 9114: HTTP/3"
[2]: https://www.rfc-editor.org/rfc/rfc9000

Deployment notes: most CDNs advertise HTTP/3 with an `Alt-Svc` header, and
browsers race it against TCP the first time [2]. See [QPACK][3] for the details
of the new header format, and [the QUIC working group][quic].

> UDP is blocked on some networks, so clients fall back to HTTP/2 over TCP [1].

[3]: https://www.rfc-editor.org/rfc/rfc9204
[rtt]: https://www.rfc-editor.org/rfc/rfc9001#section-4.6
[quic]: https://quicwg.org/
