//! 渲染预览：生成一张 650x1220 的音乐卡片样例，保存为 `preview.jpg`。
//!
//! 用法（在 `plugins/musiccard` 目录下）：
//!
//! ```text
//! cargo run --example render_preview                                   # 默认下载《晴天》真实封面，失败回退本地 / 占位图
//! cargo run --example render_preview -- D:\pictures\cover.jpg          # 指定本地封面（jpeg/png）
//! cargo run --example render_preview -- --url https://p1.music.126.net/xxx.jpg
//! cargo run --example render_preview -- --out D:\tmp\card.jpg
//! ```
//!
//! 默认输出 `plugins/musiccard/preview.jpg`，并打印封面来源、输出尺寸与字节数。
//!
//! 说明：`src/render` 在 `lib.rs` 里是私有模块，
//! 所以这里用 `#[path]` 直接复用同一份渲染源码（[`model`] + [`render`]），
//! 保证预览与插件内部走的是完全同一套实现。等 `lib.rs` 改成 `pub mod render;`
//! 后，这里可以简化为 `use musiccard::render::{render_card, Card};`。

#[path = "../src/model.rs"]
mod model;
#[path = "../src/render/mod.rs"]
mod render;

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use image::RgbImage;
use model::LyricLine;
use render::Card;

/// 示例歌曲：周杰伦《晴天》（网易云 id 186016）。
const SONG_TITLE: &str = "晴天";
const SONG_ARTISTS: &str = "周杰伦";
const SONG_ALBUM: &str = "叶惠美";
const SONG_URL: &str = "https://music.163.com/song?id=186016";

/// 兜底的在线封面：周杰伦《晴天》（网易云 id 186016 的专辑封面）。
///
/// 实测该地址返回 PNG（1080x1080），顺便覆盖「封面是 PNG + 带 alpha」的路径；
/// 防盗链 / 链接失效 / 断网时会自动回退到本地候选与占位图。
const FALLBACK_COVER_URL: &str =
    "https://p2.music.126.net/ZGffiDQZrGj5s_hnR1CNbg==/109951165566379710.jpg";

/// 一键生成预览的命令行参数。
struct Args {
    cover_path: Option<PathBuf>,
    cover_url: Option<String>,
    output: PathBuf,
}

fn usage() -> String {
    format!(
        "渲染 650x1220 音乐卡片预览\n\
         \n\
         用法：cargo run --example render_preview -- [选项]\n\
         \n\
         选项：\n\
           <path>              本地封面图片路径（jpeg/png）\n\
           --cover <path>      同上\n\
           --url <url>         用 ureq 下载封面（带网易云 UA / Referer）\n\
           --out <path>        输出路径，默认 {}\n\
           -h, --help          显示本帮助\n",
        default_output().display()
    )
}

fn default_output() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("preview.jpg")
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args {
        cover_path: None,
        cover_url: None,
        output: default_output(),
    };
    let mut iter = std::env::args().skip(1);
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "-h" | "--help" => return Err(usage()),
            "--cover" => {
                args.cover_path = Some(PathBuf::from(iter.next().ok_or("--cover 后面缺少路径")?));
            }
            "--url" => {
                args.cover_url = Some(iter.next().ok_or("--url 后面缺少地址")?);
            }
            "--out" => {
                args.output = PathBuf::from(iter.next().ok_or("--out 后面缺少路径")?);
            }
            other if other.starts_with('-') => {
                return Err(format!("未知参数：{other}\n\n{}", usage()));
            }
            other => args.cover_path = Some(PathBuf::from(other)),
        }
    }
    Ok(args)
}

/// 下载封面字节（与 netease 模块相同的 UA / Referer / 超时策略）。
fn download(url: &str) -> Result<Vec<u8>, String> {
    let response = ureq::get(url)
        .set(
            "User-Agent",
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) \
             Chrome/120.0.0.0 Safari/537.36",
        )
        .set("Referer", "https://music.163.com/")
        .timeout(Duration::from_secs(20))
        .call()
        .map_err(|e| format!("请求失败：{e}"))?;
    let mut bytes = Vec::new();
    response
        .into_reader()
        .read_to_end(&mut bytes)
        .map_err(|e| format!("读取响应失败：{e}"))?;
    if bytes.is_empty() {
        return Err("响应为空".to_string());
    }
    Ok(bytes)
}

/// 依次尝试的本地封面路径（相对 crate 根目录）。
fn local_cover_candidates() -> Vec<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    vec![
        root.join("preview-cover.jpg"),
        root.join("../../.spike/fixtures/cover.jpg"),
        root.join("../../.spike/fixtures/cover.png"),
    ]
}

/// 找不到任何真实封面时的兜底：合成一张紫粉色渐变图（保证离线也能跑通）。
fn placeholder_cover() -> Vec<u8> {
    let size = 800u32;
    let mut image = RgbImage::new(size, size);
    for (x, y, pixel) in image.enumerate_pixels_mut() {
        let tx = x as f32 / (size - 1) as f32;
        let ty = y as f32 / (size - 1) as f32;
        let r = 96.0 + 120.0 * tx + 20.0 * ty;
        let g = 70.0 + 40.0 * (1.0 - ty);
        let b = 150.0 + 70.0 * (1.0 - tx);
        pixel.0 = [
            r.clamp(0.0, 255.0) as u8,
            g.clamp(0.0, 255.0) as u8,
            b.clamp(0.0, 255.0) as u8,
        ];
    }
    // 没有额外依赖的 PNG 编码（image 的 png feature 已启用）。
    let mut bytes = Vec::new();
    let mut cursor = std::io::Cursor::new(&mut bytes);
    image::DynamicImage::ImageRgb8(image)
        .write_to(&mut cursor, image::ImageFormat::Png)
        .expect("占位封面编码失败");
    bytes
}

/// 解析封面：显式路径 > 显式 URL > 在线真实封面 > 本地候选 > 占位图。
///
/// 默认走「在线真实封面」，这样不带参数运行 example 得到的就是网易云《晴天》
/// 的真实封面（与仓库里的 preview.jpg 一致）；断网 / 防盗链拦截时才退回本地图
/// 与合成占位图。
fn resolve_cover(args: &Args) -> (Vec<u8>, String) {
    if let Some(path) = &args.cover_path {
        match std::fs::read(path) {
            Ok(bytes) if !bytes.is_empty() => {
                return (bytes, format!("本地文件 {}", path.display()));
            }
            Ok(_) => eprintln!("封面文件为空：{}", path.display()),
            Err(error) => eprintln!("读取封面失败（{}）：{error}", path.display()),
        }
    }
    if let Some(url) = &args.cover_url {
        match download(url) {
            Ok(bytes) => return (bytes, format!("在线 {url}")),
            Err(error) => eprintln!("下载封面失败（{url}）：{error}"),
        }
    }
    match download(FALLBACK_COVER_URL) {
        Ok(bytes) => return (bytes, format!("在线兜底 {FALLBACK_COVER_URL}")),
        Err(error) => eprintln!("在线兜底封面失败（{error}），改用本地候选"),
    }
    for path in local_cover_candidates() {
        match std::fs::read(&path) {
            Ok(bytes) if !bytes.is_empty() => {
                return (bytes, format!("本地文件 {}", path.display()));
            }
            _ => {}
        }
    }
    (placeholder_cover(), "合成占位图".to_string())
}

/// 示例歌词（《晴天》35% 附近，含两条翻译以覆盖「原文 + 译文」版式）。
fn sample_lyrics() -> Vec<LyricLine> {
    vec![
        LyricLine {
            text: "故事的小黄花".to_string(),
            translation: Some("The little yellow flower of the story".to_string()),
        },
        LyricLine {
            text: "从出生那年就飘着".to_string(),
            translation: None,
        },
        LyricLine {
            text: "童年的荡秋千".to_string(),
            translation: Some("The swing of childhood".to_string()),
        },
        LyricLine {
            text: "随记忆一直晃到现在".to_string(),
            translation: None,
        },
        LyricLine {
            text: "Re So So Si Do Si La".to_string(),
            translation: None,
        },
    ]
}

fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(args) => args,
        Err(message) => {
            // --help 也走这里：帮助不是错误，但退出码不必为 0 之外的复杂情况。
            println!("{message}");
            return ExitCode::SUCCESS;
        }
    };

    let (cover, cover_source) = resolve_cover(&args);
    let lyrics = sample_lyrics();

    let card = Card {
        title: SONG_TITLE,
        artists: SONG_ARTISTS,
        album: SONG_ALBUM,
        cover: &cover,
        qr_content: SONG_URL,
        month: "Nov",
        day: "04",
        lyrics: &lyrics,
    };

    println!("封面来源：{cover_source}（{} 字节）", cover.len());
    let jpeg = match render::render_card(&card) {
        Ok(bytes) => bytes,
        Err(error) => {
            eprintln!("渲染失败：{error}");
            return ExitCode::FAILURE;
        }
    };

    if let Some(parent) = args.output.parent()
        && !parent.as_os_str().is_empty()
        && let Err(error) = std::fs::create_dir_all(parent)
    {
        eprintln!("创建输出目录失败：{error}");
        return ExitCode::FAILURE;
    }
    if let Err(error) = std::fs::write(&args.output, &jpeg) {
        eprintln!("写入预览图失败：{error}");
        return ExitCode::FAILURE;
    }

    // 自检：尺寸必须与契约一致（650x1220）。
    let decoded = image::load_from_memory(&jpeg).expect("预览图应可解码");
    println!(
        "输出：{}（{}x{}，{} 字节，JPEG q90）",
        args.output.display(),
        decoded.width(),
        decoded.height(),
        jpeg.len()
    );
    ExitCode::SUCCESS
}
