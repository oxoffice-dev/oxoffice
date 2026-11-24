# Test signing key

`test-signing-key.pem` and `test-signing-cert.pem` are a throwaway RSA 2048
key (PKCS#8, unencrypted) and its self-signed certificate. They exist only so
that the docx and pptx tests can sign packages with `sign`. They protect
nothing: never trust them, never use them outside the tests.

They are committed rather than generated in the tests because opc signs with
`ring`, which cannot generate RSA keys, and nothing else in the dependency tree
can.

Made with OpenSSL 3.5:

```bash
openssl req -x509 -newkey rsa:2048 -nodes -sha256 -days 36500 \
  -subj "/CN=oxoffice test signer, test only" \
  -keyout test-signing-key.pem -out test-signing-cert.pem
```
