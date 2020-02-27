use dtn7::dtnconfig::DtnConfig;
use tinytemplate::TinyTemplate;
#[macro_use]
extern crate serde_derive;

#[derive(Serialize)]
struct Context<'a> {
    config: &'a DtnConfig,
    num_peers: u64,
    num_bundles: u64,
}
#[test]
fn template_test() {
    let template_str = include_str!("../webroot/index.html");
    let mut tt = TinyTemplate::new();
    tt.add_template("index", template_str).unwrap();
    let cfg = DtnConfig::new();
    let context = Context {
        config: &cfg,
        num_peers: 4,
        num_bundles: 10,
    };

    let rendered = tt.render("index", &context).unwrap();
    println!("{}", rendered);
}
