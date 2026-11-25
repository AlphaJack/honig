<!-- Generated from honig/src/server/range.rs by src/tests/docs_pages.rs. Edit the source, not this file. -->

Range requests (RFC 9110 section 14): one byte range of a binary response, for a download that resumes or a
player that seeks.

The server answers `Range` itself on a `GET` whose success response is binary, such as `application/zip` or an
image. Your handler returns the whole body as usual:

| Request | Response |
|---|---|
| No `Range` | `200` with `Accept-Ranges: bytes` |
| `Range: bytes=1000-`, `bytes=0-499` or `bytes=-500` | `206 Partial Content`: those bytes and `Content-Range: bytes 1000-4999/5000` |
| A range that starts past the end | `416 Range Not Satisfiable` with `Content-Range: bytes */5000` |
| `If-Range` next to it with an `ETag` or date that the body no longer has | `200`: the whole new body |

Only a body whose length is known can be cut. That is a body in memory (shared bytes of a `crate::Blob` are cut
without a copy) or a `crate::Blob::ranged`. The server calls the `open(first, last)` of a ranged blob once with
the bytes it sends. So the bytes before the range are never loaded.

These go out whole with no `Accept-Ranges`, as RFC 9110 allows: a `crate::Blob::stream` of unknown length, JSON
and other structured bodies, several ranges in one request and a `Range` that does not parse. An operation that
declares a `206` (or `2XX`) or a `Range` parameter of its own answers ranges in its handler
(`Op::ranges`). The server leaves it alone.
