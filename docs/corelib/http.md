# `core.http` — HTTP client

Import with `<< core.http`. See the [corelib index](../LANGUAGE.md#corelib).

A minimal HTTP client written in Quilon over [`core.net`](net.md)'s `@tcpRequest`. **HTTP only, no TLS** — URLs are `http://host[:port]/path` (default port 80). Each request opens one connection, sends `Connection: close`, and reads the close-delimited reply.

## Types

| Type | Shape |
|------|-------|
| `Body` | `{ content :: Text, contentType :: Text }` |
| `Method` | `Get / Post(Body) / Put(Body) / Query(Body) / Delete / Head` — the body-bearing methods carry a `Body`. |
| `Request` | `{ method :: Method, url :: Text }` |
| `Response` | `{ raw :: Text }` — read fields with `status` / `header` / `body`. |

## Functions

| Function | Result |
|----------|--------|
| `get(url :: Text) -> Result` | Build a GET `Request` for `url` and `send` it: `Ok(Response)` / `NotOk(Text)`. |
| `send(request :: Request) -> Result` | Perform `request` over `core.net` and parse the reply; a transport failure comes back as `NotOk(Text)`. |
| `requestText(request :: Request) -> Text` | The raw HTTP/1.0 request bytes for `request`. |
| `parseResponse(raw :: Text) -> Result` | Wrap raw bytes as a `Response`, checking the status line is present. |
| `status(response :: Response) -> Num` | The status code (`HTTP/1.0 200 OK` → `200`). |
| `header(response :: Response, name :: Text) -> Result` | A header value by name, case-insensitive: `Ok(Text)` / `NotOk(Text)`. |
| `body(response :: Response) -> Text` | Everything after the blank line. |

`Query` serialises like `Post` (method token `QUERY`, body, `Content-Length`); its `Accept-Query` reply header reads back through `header`.

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

`examples/http_parse.qn` builds and parses offline (no network); `examples/http_get.qn` makes a live GET.
