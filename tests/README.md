# Test fixtures

`fixtures/` holds throwaway certificates made with openssl and keytool (password `changeit`):
a client key with its chain as classic JKS and as PKCS12, a trust store in both formats, and PEM files.
They are for tests only and protect nothing.

The ignored test `connects_to_tls_server_every_way` expects a MongoDB on `localhost:27017` that
requires TLS client certificates signed by the test CA, with user `app_user` / `s3cret!`.
The matching server certificate is not checked in; generate a new set of fixtures if you need to run it.
