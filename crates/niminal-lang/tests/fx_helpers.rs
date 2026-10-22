use niminal_lang::compile;

#[test]
fn stateless_helpers_return_values_when_called_from_effects() {
    let source = "fn half(x) { x * 0.5 }\nfn soften(x) { half(x) }\nfx low { out = soften(it) }";
    assert!(compile(source).is_ok());
    let errors = compile("fx low { it * 0.5 }").err().unwrap();
    assert!(errors.iter().any(|e| e.message.contains("through `out`")));
}
