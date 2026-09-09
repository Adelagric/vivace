fn main() {
    let text = std::env::args().nth(1).expect("json arg");
    let v: serde_json::Value = serde_json::from_str(&text).expect("parse");
    println!(
        "{}",
        vivace_core::phpjson::php_json_encode(&v).expect("enc")
    );
}
