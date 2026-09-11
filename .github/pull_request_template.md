## Nereids contribution

### Type

- [ ] Tokenizer adapter
- [ ] Core bridge
- [ ] Test / fixture
- [ ] Documentation
- [ ] Benchmark
- [ ] Other

### Adapter / model

Tokenizer/model family:

Runtime/library:

Version or provenance:

### Exactness gate

- [ ] Source native tokens reconstruct exact raw bytes
- [ ] DU representation reconstructs exact raw bytes
- [ ] Target native tokens reconstruct exact raw bytes
- [ ] Byte offsets are byte offsets, not character offsets
- [ ] Gaps are rejected
- [ ] Overlaps are rejected
- [ ] Incorrect token bytes are rejected
- [ ] Multibyte UTF-8 is covered

### Model validation

- [ ] Conformance-only / mock
- [ ] Tested against a real local tokenizer/model
- [ ] Receipt attached

Input SHA256:

Model/tokenizer provenance:

Receipt path or artifact:

### Tests

```text
cargo fmt --check
cargo test --all-targets
cargo build --release
```

Result:

### Claims

Do not claim model compatibility from mocks alone.

Do not describe learned embedding, hidden-state, or KV projection as exact.

Nereids exactness applies to verified byte/span/tokenizer reconstruction.
