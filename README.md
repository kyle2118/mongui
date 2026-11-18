# mongui

A small, fast MongoDB browser written in Rust. It does a handful of things and nothing else:

- Saved connections, with colors, import from Compass, and export/import for sharing
- TLS with a **JKS keystore** (the same file the Spring Boot apps use), or with PEM files as in Compass
- Browse, filter and sort documents, with paging that stays fast on collections with millions of documents
- Edit, insert, copy and delete documents
- Database and collection stats

## Run it

You need Rust once: `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh`

```
cargo run --release
```

The first build takes a few minutes. After that the binary is at `target/release/mongui`.
To get a double-clickable `mongui.app` on macOS, run `./scripts/make-app.sh`.
The app icon comes from `assets/mongui-icon.png` (1024x1024, transparent background); the script turns it
into `mongui.icns` with the built-in `sips` and `iconutil` tools, and the binary embeds the same PNG for the Dock/window icon.

## Connecting

| Setting | What to pick |
| --- | --- |
| Client certificate | **JKS keystore** and its password, or **PEM file** (certificate + key in one file) |
| Server CA | **cacerts / trust store**: the cacerts file you copy into each JDK. **CA .pem**: the Certificate Authority file from Compass. **From client keystore**: uses CA certificates stored inside the JKS. **System**: this computer's trusted roots |
| Sign in | Username/password (auth database defaults to `admin`), or LDAP for a personal login |

PKCS12 files (`.p12`, or a `.jks` made by a recent `keytool`) work in the same fields.

The MongoDB driver can only read certificates from PEM files, so when a JKS is used mongui writes a
PEM copy into a private temp folder (owner-only permissions), hands it to the driver, and deletes it
right after the connection is created.

## Where things are stored

Connections live in `~/Library/Application Support/mongui/connections.json`, readable only by you.
Passwords are stored in that file in plain text (no Keychain), so treat it like the `.jks` files.

**Export** always removes LDAP passwords. Shared service-account and keystore passwords are included
unless you untick the box. Certificate files are not copied, only their paths.

**Import from Compass** reads Compass's saved (favorite) connections. Compass keeps passwords in the
macOS Keychain; tick "Also copy the saved passwords" and macOS will ask for permission, or leave it
off and type them in.

## Filters and editing

Filters and sorts use shell syntax: `{ status: 'READY', total: { $gt: 90 } }`, `{ createdAt: -1 }`.
Supported helpers: `ObjectId("…")`, `ISODate("…")`, `NumberLong(…)`, `NumberInt(…)`,
`NumberDecimal("…")`, `UUID("…")`, `BinData(…)`, `Timestamp(…)`, `/regex/i`. Extended JSON works too.

The editor keeps types exactly as written: `42` is Int32, `NumberLong(42)` is Int64, `42.0` is a Double.

## Big collections

- Each page is one small query (`skip` + `limit`), never the whole collection.
- The pager reads `1 2 3 … current go-to last`, then "total N" and the query time.
- With no filter, the total comes from collection metadata (instant, an estimate; hover "total" to see that).
- With a filter, the exact count gets 12 seconds; after that paging falls back to previous/next.
- Every query is cancelled on the server after 30 seconds, so a bad filter cannot hang anything.
- Jumping to a very deep page uses `skip`, which the server has to walk. It is fine for occasional use.

## Tests

`cargo test` runs the parser, keystore and import/export tests.
`cargo test -- --include-ignored` also connects to a local TLS server, see `tests/README.md`.
