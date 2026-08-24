# `core.http` — HTTP client

Import with `<< core.http`. See the [corelib index](../LANGUAGE.md#corelib).

A minimal HTTP client written entirely in Quilon on top of [`core.net`](net.md)'s
`@tcpRequest`. Each request opens one connection, sends `Connection: close`, and reads the
close-delimited reply. **HTTP only — no TLS**, so URLs are plain `http://…` (default port
80). `Request` and `Response` are rich-but-lazy: they hold the raw data and parse fields
when an accessor reads them.

## Types

| Type | Shape | Meaning |
|------|-------|---------|
| `Body` | `{ content :: Text, contentType :: Text }` | A request body and the media type to advertise. |
| `Method` | `Get / Post(Body) / Put(Body) / Delete / Head` | The HTTP method; the body-bearing methods carry a `Body`. |
| `Request` | `{ method :: Method, url :: Text }` | A request: the method and target URL (`http://host[:port]/path`, scheme optional). |
| `Response` | `{ raw :: Text }` | A reply: the raw bytes. Read fields with `status` / `header` / `body`. |

## Functions

| Function | Result | Effect |
|----------|--------|--------|
| `get(url :: Text) -> Result` | `Ok(Response)` / `NotOk(Text)` | Build a GET `Request` for `url` and `send` it. |
| `send(request :: Request) -> Result` | `Ok(Response)` / `NotOk(Text)` | Perform `request` over `core.net` and parse the reply. |
| `requestText(request :: Request) -> Text` | — | Serialise a `Request` into the raw HTTP/1.0 request bytes sent over the wire. |
| `parseResponse(raw :: Text) -> Result` | `Ok(Response)` / `NotOk(Text)` | Wrap raw response bytes as a `Response`, validating the status line is present. |
| `status(response :: Response) -> Num` | — | The numeric status code from the status line (`HTTP/1.0 200 OK` → `200`). |
| `header(response :: Response, name :: Text) -> Result` | `Ok(Text)` / `NotOk(Text)` | Look up a header by name (case-insensitive). |
| `body(response :: Response) -> Text` | — | Everything after the blank line separating headers from body. |

```quilon
<< core.http
<< core.test

^ = () -> $ => <
  page :: Response = get("http://example.com/") ?
    | Ok(response) => response
    | NotOk(_)     => Response { raw = "" }
  assertEq(status(page), 200)
  assert(body(page).contains("Example Domain"))
>
```

`send` propagates a transport failure from `@tcpRequest` as `NotOk(error)`, so a network
error never crashes the program — match the `Result` to handle it. `examples/http_parse.ql`
exercises the offline half (building requests, parsing responses) with no network;
`examples/http_get.ql` makes a live GET.
