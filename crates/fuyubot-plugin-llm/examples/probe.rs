//! 命令行测试接口：把三个协议都跑一遍，验证地址 / Key / 请求体。
//!
//! 用法（在仓库根）：
//!
//! ```bash
//! export FUYUBOT_LLM_BASE_URL=https://api.deepseek.com
//! export FUYUBOT_LLM_API_KEY=sk-…
//!
//! cargo run -p fuyubot-plugin-llm --example probe -- --protocol models
//! cargo run -p fuyubot-plugin-llm --example probe -- --protocol chat --prompt "你好"
//! cargo run -p fuyubot-plugin-llm --example probe -- --protocol responses --prompt "你好" --stream
//! cargo run -p fuyubot-plugin-llm --example probe -- --protocol messages --prompt "你好"
//! cargo run -p fuyubot-plugin-llm --example probe -- --protocol test
//! cargo run -p fuyubot-plugin-llm --example probe -- --protocol chat --json body.json
//! ```
//!
//! `--json` 直接读一份协议原生 body，适合验证未建模字段（tools / vision / 结构化输出等）。

use std::env;
use std::error::Error;
use std::fs;

use llm::{ClientConfig, LlmClient, LlmError, Protocol};
use serde_json::{Value, json};

fn main() {
    if let Err(error) = run() {
        eprintln!("probe 失败：{error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let mut protocol_name = "test".to_owned();
    let mut model = env::var("FUYUBOT_LLM_MODEL").unwrap_or_else(|_| "deepseek-chat".to_owned());
    let mut prompt = "Reply with exactly: pong".to_owned();
    let mut system: Option<String> = None;
    let mut max_tokens = 256u64;
    let mut temperature = 0.0f64;
    let mut stream = false;
    let mut json_file: Option<String> = None;
    let mut config = ClientConfig::from_env();

    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--protocol" => protocol_name = args.next().ok_or("--protocol 缺值")?,
            "--model" => model = args.next().ok_or("--model 缺值")?,
            "--prompt" => prompt = args.next().ok_or("--prompt 缺值")?,
            "--system" => system = Some(args.next().ok_or("--system 缺值")?),
            "--max-tokens" => max_tokens = args.next().ok_or("--max-tokens 缺值")?.parse()?,
            "--temperature" => temperature = args.next().ok_or("--temperature 缺值")?.parse()?,
            "--stream" => stream = true,
            "--json" => json_file = Some(args.next().ok_or("--json 缺值")?),
            "--base-url" => config.base_url = args.next().ok_or("--base-url 缺值")?,
            "--api-key" => config.api_key = args.next().ok_or("--api-key 缺值")?,
            "--debug" => config.debug = true,
            "--help" | "-h" => {
                print_help();
                return Ok(());
            }
            other => return Err(format!("未知参数：{other}").into()),
        }
    }

    let client = LlmClient::new(config)?;
    if !client.config().has_api_key() {
        return Err("缺少 API Key：设置 FUYUBOT_LLM_API_KEY 或传 --api-key".into());
    }

    match protocol_name.as_str() {
        "models" => {
            let value = client.models()?;
            println!("{}", serde_json::to_string_pretty(&value)?);
        }
        "test" => {
            let report = client.test(&model);
            println!("{}", serde_json::to_string_pretty(&report)?);
            if !report.ok {
                return Err("至少一个协议自检失败".into());
            }
        }
        other => {
            let protocol = Protocol::parse(other)
                .ok_or_else(|| LlmError::UnsupportedProtocol(other.to_owned()))?;
            let mut body = match json_file {
                Some(path) => fs::read_to_string(&path)
                    .map_err(|error| format!("读 {path} 失败：{error}"))?
                    .parse::<Value>()?,
                None => build_body(
                    protocol,
                    &model,
                    &prompt,
                    system.as_deref(),
                    max_tokens,
                    temperature,
                ),
            };
            if stream {
                set_stream(&mut body);
                let mut stream = client.request_stream(protocol, &body)?;
                let mut count = 0u64;
                for event in stream.by_ref() {
                    count += 1;
                    println!("{}", serde_json::to_string(&event?)?);
                }
                eprintln!("probe: {protocol} 流式结束，共 {count} 条事件");
            } else {
                let value = client.request_json(protocol, &body)?;
                println!("{}", serde_json::to_string_pretty(&value)?);
            }
        }
    }

    Ok(())
}

fn build_body(
    protocol: Protocol,
    model: &str,
    prompt: &str,
    system: Option<&str>,
    max_tokens: u64,
    temperature: f64,
) -> Value {
    match protocol {
        Protocol::OpenAiChat => {
            let mut messages = Vec::new();
            if let Some(system) = system {
                messages.push(json!({ "role": "system", "content": system }));
            }
            messages.push(json!({ "role": "user", "content": prompt }));
            json!({
                "model": model,
                "messages": messages,
                "max_tokens": max_tokens,
                "temperature": temperature,
            })
        }
        Protocol::OpenAiResponses => {
            let mut body = json!({
                "model": model,
                "input": prompt,
                "max_output_tokens": max_tokens,
            });
            if let Some(system) = system {
                body["instructions"] = Value::String(system.to_owned());
            }
            body
        }
        Protocol::AnthropicMessages => {
            let mut body = json!({
                "model": model,
                "max_tokens": max_tokens,
                "messages": [{ "role": "user", "content": prompt }],
            });
            if let Some(system) = system {
                body["system"] = Value::String(system.to_owned());
            }
            body
        }
    }
}

fn set_stream(body: &mut Value) {
    if let Some(object) = body.as_object_mut() {
        object.insert("stream".to_owned(), Value::Bool(true));
    }
}

fn print_help() {
    println!(
        r#"fuyubot-plugin-llm probe

  --protocol <chat|responses|messages|models|test>  默认 test
  --model <name>                                    默认 FUYUBOT_LLM_MODEL 或 deepseek-chat
  --prompt <text>
  --system <text>
  --max-tokens <n>
  --temperature <f>
  --stream
  --json <file>          直接使用协议原生 JSON body
  --base-url <url>       覆盖 FUYUBOT_LLM_BASE_URL
  --api-key <key>        覆盖 FUYUBOT_LLM_API_KEY
  --debug
"#
    );
}
