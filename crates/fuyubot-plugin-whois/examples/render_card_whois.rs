//! 生成一张 Whois 卡片要用的 html2img 请求 JSON，用来调样式 / 做验收。
//!
//! 界面在 `assets/card.html`（Vue 模板 + 内联 CSS）；渲染由宿主的
//! `fuyubot.html2img` 服务完成。这里把 `{ html, vue, data, … }` 落盘，装好 html2img
//! 的宿主 / 调试环境可以直接拿它复现。
//!
//! ```bash
//! # 默认 1200px 宽，输出 render-sample.json
//! cargo run --example render_card_whois
//! # 指定输出与宽度
//! cargo run --example render_card_whois -- render-900.json 900
//! ```

use std::path::PathBuf;
use std::process::ExitCode;

use whois::core::model::CardData;
use whois::render::{TEMPLATE, clamp_width, view_data};

/// 一份典型的已注册域名数据（字段与接口返回一一对应）。
fn sample_card() -> CardData {
    CardData {
        is_available: 0,
        domain: "example.com".to_owned(),
        domain_suffix: ".com".to_owned(),
        query_time: "2026-11-12 17:30:00".to_owned(),
        registrant_name: "REDACTED FOR PRIVACY".to_owned(),
        registrant_email: "whois@example.com".to_owned(),
        registrar_name: "Example Registrar, Inc.".to_owned(),
        creation_time: "1995-08-14 04:00:00".to_owned(),
        expiration_time: "2027-08-13 04:00:00".to_owned(),
        creation_days: 11_416,
        valid_days: 274,
        is_expire: 0,
        domain_status: vec!["clientDeleteProhibited （注册商设置禁止删除）".to_owned()],
        name_server: vec![
            "a.iana-servers.net".to_owned(),
            "b.iana-servers.net".to_owned(),
        ],
        whois_server: "whois.verisign-grs.com".to_owned(),
        raw: String::new(),
        ip: "93.184.216.34".to_owned(),
        country: "美国".to_owned(),
        city: "洛杉矶".to_owned(),
        area: String::new(),
        isp: "Example ISP".to_owned(),
    }
}

fn main() -> ExitCode {
    let mut output = PathBuf::from("render-sample.json");
    let mut width: u32 = 1200;

    for arg in std::env::args().skip(1) {
        if arg.ends_with(".json") || arg.ends_with(".JSON") {
            output = PathBuf::from(arg);
        } else if let Ok(parsed) = arg.parse::<u32>() {
            // 纯数字参数当宽度，方便试窄卡片（900 / 1200 / 1600）。
            width = parsed;
        }
    }

    let width = clamp_width(width);
    let payload = serde_json::json!({
        "html": TEMPLATE,
        "vue": true,
        "data": view_data(&sample_card(), width, None),
        "width": width,
        "height": 200,
        "scale": 1.0,
        "format": "jpg",
        "auto_height": true,
    });

    match serde_json::to_vec_pretty(&payload) {
        Ok(bytes) => {
            if let Err(error) = std::fs::write(&output, bytes) {
                eprintln!("写 {} 失败: {error}", output.display());
                return ExitCode::FAILURE;
            }
            println!(
                "已输出 {}（模板 {} 字节，宽度 {}px）",
                output.display(),
                TEMPLATE.len(),
                width
            );
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("序列化失败: {error}");
            ExitCode::FAILURE
        }
    }
}
