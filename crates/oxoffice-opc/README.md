# oxoffice-opc

Read, mutate, and write the Open Packaging Conventions layer used by OOXML
files.

## Capabilities

- ZIP package parts with configurable read limits.
- Content type defaults and overrides.
- Relationship parsing, target resolution, and mutation.
- Main-document discovery, part navigation, and package preservation.
- Case-insensitive package identity, atomic save, agile encryption, and XML
  signature verification cover the container boundary below both facades.

## Use it when

Use this crate when implementing an OOXML package reader or writer. Use `rdocx` or `rpptx` for complete document APIs.

## Relationship

This is the shared package layer used by
both document families. OPC manages containers, parts, and relationships. It
does not interpret document-format XML. Encryption and digital signature APIs
are optional feature-gated capabilities.

## Example

```rust,no_run
use oxoffice_opc::ContentTypes;

let content_types = ContentTypes::from_xml(br#"
    <Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
      <Default Extension="xml" ContentType="application/xml"/>
    </Types>
"#)?;
assert_eq!(content_types.defaults["xml"], "application/xml");
# Ok::<(), oxoffice_opc::OpcError>(())
```

Add `oxoffice-opc = "0.15.0"` to your dependencies. Start with the [package API documentation](https://docs.rs/oxoffice-opc).
