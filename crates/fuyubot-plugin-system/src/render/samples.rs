//! 内置样张：不联网、不读外部文件也能出图，供 example、单测和调样式用。
//!
//! 覆盖这几种典型版面（也是「极端数据不 panic」的回归用例）：
//!
//! | 函数 | 覆盖点 |
//! | --- | --- |
//! | [`sample_card`] | 有 admin、CPU/RAM 两条 gauge、4 块磁盘、10 个插件（最常见的完整形态） |
//! | [`sample_card_no_admin`] | 无 admin（配置提示 + 头像占位）、2 条 gauge、1 块磁盘、6 个插件 |
//! | [`sample_card_long_names`] | 超长机器人名 / 用户名、长插件名（截断与缩字号）、3 块磁盘 |
//! | [`sample_card_empty`] | 全空数据（三处「暂无数据」兜底） |
//!
//! [`sample_assets`] 现场画 3 张纯色 / 渐变小图当头像与背景——所有内容都由
//! `image` 在内存里合成，不依赖磁盘上的任何文件。

use image::{ImageFormat, Rgb, RgbImage};

use crate::core::model::{Assets, CardData, DiskUsage, Gauge, Identity, PluginChip, TdlibInfo};

/// 完整样张：机器人 + 管理员 + CPU/RAM 两条 gauge + 四块磁盘 + 十个插件。
pub fn sample_card() -> CardData {
    CardData {
        bot: Identity {
            name: "Fuyu 智能助手".to_owned(),
            username: "fuyubot".to_owned(),
            id: 7_834_129_905,
        },
        admin: Some(Identity {
            name: "柚子".to_owned(),
            username: "yuzu".to_owned(),
            id: 5_483_217_709,
        }),
        tdlib: TdlibInfo {
            version: "1.8.29".to_owned(),
            commit_hash: "47b3d5a0a1cc0f3c1f4b9c5e0f0f2a1e6d3b8c7a".to_owned(),
        },
        gauges: vec![
            Gauge {
                label: "CPU".to_owned(),
                usage: 37,
            },
            Gauge {
                label: "RAM".to_owned(),
                usage: 62,
            },
        ],
        disks: vec![
            DiskUsage {
                label: "C".to_owned(),
                used: "218.42 GB".to_owned(),
                total: "476.94 GB".to_owned(),
                usage_percent: 46,
            },
            DiskUsage {
                label: "D".to_owned(),
                used: "812.05 GB".to_owned(),
                total: "1.81 TB".to_owned(),
                usage_percent: 44,
            },
            DiskUsage {
                label: "E".to_owned(),
                used: "1.24 TB".to_owned(),
                total: "1.81 TB".to_owned(),
                usage_percent: 68,
            },
            DiskUsage {
                label: "F".to_owned(),
                used: "96.31 GB".to_owned(),
                total: "931.51 GB".to_owned(),
                usage_percent: 10,
            },
        ],
        plugins: vec![
            chip("system", "active"),
            chip("musiccard", "active"),
            chip("whois", "active"),
            chip("kfc", "active"),
            chip("epic", "pending"),
            chip("weather", "failed"),
            chip("anime", "active"),
            chip("bilibili", "active"),
            chip("chat", "active"),
            chip("help", "active"),
        ],
    }
}

/// 没有管理员：admin 面板走配置提示，CPU/RAM 两条 gauge、1 块磁盘、6 个插件。
pub fn sample_card_no_admin() -> CardData {
    CardData {
        bot: Identity {
            name: "Fuyu 智能助手".to_owned(),
            username: "fuyubot".to_owned(),
            id: 7_834_129_905,
        },
        admin: None,
        tdlib: TdlibInfo {
            version: "1.8.29".to_owned(),
            commit_hash: "47b3d5a0a1cc0f3c1f4b9c5e0f0f2a1e6d3b8c7a".to_owned(),
        },
        gauges: vec![
            Gauge {
                label: "CPU".to_owned(),
                usage: 8,
            },
            Gauge {
                label: "RAM".to_owned(),
                usage: 91,
            },
        ],
        disks: vec![DiskUsage {
            label: "C".to_owned(),
            used: "412.55 GB".to_owned(),
            total: "476.94 GB".to_owned(),
            usage_percent: 86,
        }],
        plugins: vec![
            chip("system", "active"),
            chip("musiccard", "active"),
            chip("whois", "active"),
            chip("kfc", "active"),
            chip("epic", "pending"),
            chip("weather", "failed"),
        ],
    }
}

/// 超长文本样张：名字、用户名、插件名都往死里长，验证「缩字号 / 截断」不溢出。
pub fn sample_card_long_names() -> CardData {
    CardData {
        bot: Identity {
            name: "Fuyu 是一个名字特别特别长的机器人用来测试截断逻辑是否可靠".to_owned(),
            username: "a_very_long_telegram_username_that_should_be_truncated".to_owned(),
            id: 7_834_129_905,
        },
        admin: Some(Identity {
            name: "这是一位名字也非常长的超级管理员".to_owned(),
            username: "super_admin_with_a_long_username".to_owned(),
            id: 5_483_217_709,
        }),
        tdlib: TdlibInfo {
            version: "1.8.29".to_owned(),
            commit_hash: "47b3d5a0a1cc0f3c1f4b9c5e0f0f2a1e6d3b8c7a9e1f2c3d4b5a6".to_owned(),
        },
        gauges: vec![
            Gauge {
                label: "CPU".to_owned(),
                usage: 100,
            },
            Gauge {
                label: "RAM".to_owned(),
                usage: 0,
            },
        ],
        disks: vec![
            DiskUsage {
                label: "系统盘（C）".to_owned(),
                used: "1.02 TB".to_owned(),
                total: "1.09 TB".to_owned(),
                usage_percent: 93,
            },
            DiskUsage {
                label: "D".to_owned(),
                used: "1023.45 GB".to_owned(),
                total: "1907.69 GB".to_owned(),
                usage_percent: 53,
            },
            DiskUsage {
                label: "E".to_owned(),
                used: "0.12 GB".to_owned(),
                total: "931.51 GB".to_owned(),
                usage_percent: 0,
            },
        ],
        plugins: (0..10)
            .map(|index| {
                chip(
                    &format!("a_very_long_plugin_name_number_{index:02}_with_suffix"),
                    if index % 4 == 3 { "failed" } else { "active" },
                )
            })
            .collect(),
    }
}

/// 全空数据：三处「暂无数据」+ 未配置管理员 + ID:0，检查兜底路径。
pub fn sample_card_empty() -> CardData {
    CardData::default()
}

/// 样张素材：机器人头像、管理员头像、背景图，全部在内存里画出来（PNG 字节）。
pub fn sample_assets() -> Assets {
    Assets {
        bot_photo: Some(bot_photo_png()),
        admin_photo: Some(admin_photo_png()),
        background: Some(background_png()),
    }
}

/// 造一枚插件胶囊。
fn chip(name: &str, state: &str) -> PluginChip {
    PluginChip {
        name: name.to_owned(),
        state: state.to_owned(),
    }
}

/// 机器人头像：256×256 蓝→青渐变 + 中间一个浅色圆（够像「机器人」即可）。
fn bot_photo_png() -> Vec<u8> {
    const SIZE: u32 = 256;
    let mut image = RgbImage::new(SIZE, SIZE);
    for (x, y, pixel) in image.enumerate_pixels_mut() {
        let t = (x as f32 + y as f32) / (2.0 * SIZE as f32);
        *pixel = Rgb([
            lerp(14.0, 34.0, t),
            lerp(165.0, 211.0, t),
            lerp(233.0, 238.0, t),
        ]);
    }
    // 中间一个白色圆环 + 圆点，模拟机器人「眼睛」。
    let center = SIZE as f32 / 2.0;
    for (x, y, pixel) in image.enumerate_pixels_mut() {
        let dx = x as f32 + 0.5 - center;
        let dy = y as f32 + 0.5 - center;
        let distance = (dx * dx + dy * dy).sqrt();
        if (72.0..=86.0).contains(&distance) {
            *pixel = Rgb([255, 255, 255]);
        } else if distance < 40.0 {
            *pixel = Rgb([15, 118, 178]);
        }
    }
    encode_png(&image)
}

/// 管理员头像：192×192 粉→橙渐变 + 中间浅色圆。
fn admin_photo_png() -> Vec<u8> {
    const SIZE: u32 = 192;
    let mut image = RgbImage::new(SIZE, SIZE);
    for (x, y, pixel) in image.enumerate_pixels_mut() {
        let t = (x as f32 + y as f32) / (2.0 * SIZE as f32);
        *pixel = Rgb([
            lerp(251.0, 250.0, t),
            lerp(113.0, 204.0, t),
            lerp(133.0, 21.0, t),
        ]);
    }
    let center = SIZE as f32 / 2.0;
    for (x, y, pixel) in image.enumerate_pixels_mut() {
        let dx = x as f32 + 0.5 - center;
        let dy = y as f32 + 0.5 - center;
        if (dx * dx + dy * dy).sqrt() < 52.0 {
            *pixel = Rgb([255, 241, 236]);
        }
    }
    encode_png(&image)
}

/// 背景图：1600×900 蓝→紫渐变 + 几个软边色块，用来验证 cover-fit 与白色蒙版的观感。
fn background_png() -> Vec<u8> {
    const WIDTH: u32 = 1600;
    const HEIGHT: u32 = 900;
    let mut image = RgbImage::new(WIDTH, HEIGHT);

    for (x, y, pixel) in image.enumerate_pixels_mut() {
        let t = (x as f32 * 0.6 + y as f32 * 0.4) / (WIDTH as f32 * 0.6 + HEIGHT as f32 * 0.4);
        *pixel = Rgb([
            lerp(59.0, 192.0, t),
            lerp(130.0, 132.0, t),
            lerp(246.0, 252.0, t),
        ]);
    }

    // 三个软边圆：黄 / 绿 / 白，纯手工混色，避免引入额外依赖。
    let blobs = [
        (
            1240.0f32,
            320.0f32,
            260.0f32,
            [251.0f32, 191.0f32, 36.0f32],
            0.55f32,
        ),
        (980.0, 720.0, 300.0, [16.0, 185.0, 129.0], 0.45),
        (1420.0, 760.0, 220.0, [255.0, 255.0, 255.0], 0.5),
    ];
    for (cx, cy, radius, color, max_alpha) in blobs {
        for (x, y, pixel) in image.enumerate_pixels_mut() {
            let dx = x as f32 + 0.5 - cx;
            let dy = y as f32 + 0.5 - cy;
            let distance = (dx * dx + dy * dy).sqrt();
            if distance >= radius {
                continue;
            }
            // 从圆心到边缘平滑降到 0。
            let alpha = (1.0 - distance / radius).powi(2) * max_alpha;
            // 只混合 RGB，保留 alpha 通道。
            for (dst, src) in pixel.0.iter_mut().take(3).zip(color) {
                *dst = (*dst as f32 * (1.0 - alpha) + src * alpha)
                    .round()
                    .clamp(0.0, 255.0) as u8;
            }
        }
    }
    encode_png(&image)
}

/// 线性插值。
fn lerp(from: f32, to: f32, t: f32) -> u8 {
    (from + (to - from) * t.clamp(0.0, 1.0))
        .round()
        .clamp(0.0, 255.0) as u8
}

/// RGB 图编码成 PNG 字节。
fn encode_png(image: &RgbImage) -> Vec<u8> {
    let mut cursor = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(image.clone())
        .write_to(&mut cursor, ImageFormat::Png)
        .expect("内置样张编码 PNG 不应失败");
    cursor.into_inner()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn samples_cover_the_required_shapes() {
        let full = sample_card();
        assert!(full.admin.is_some());
        assert_eq!(full.gauges.len(), 2, "CPU / RAM，SWAP 已去掉");
        assert_eq!(full.disks.len(), 4);
        assert!((6..=10).contains(&full.plugins.len()));

        let no_admin = sample_card_no_admin();
        assert!(no_admin.admin.is_none());
        assert_eq!(no_admin.gauges.len(), 2);
        assert_eq!(no_admin.disks.len(), 1);
        assert!((6..=10).contains(&no_admin.plugins.len()));

        let long = sample_card_long_names();
        assert!(long.bot.name.chars().count() > 20);
        assert!(long.bot.username.chars().count() > 20);
        assert_eq!(long.disks.len(), 3);
        assert!((6..=10).contains(&long.plugins.len()));

        assert!(sample_card_empty().plugins.is_empty());
    }

    #[test]
    fn assets_are_valid_images() {
        let assets = sample_assets();
        let bot = assets.bot_photo.expect("应当有机器人头像");
        let admin = assets.admin_photo.expect("应当有管理员头像");
        let background = assets.background.expect("应当有背景图");

        let bot = image::load_from_memory(&bot).expect("头像应当是合法图片");
        let admin = image::load_from_memory(&admin).expect("头像应当是合法图片");
        let background = image::load_from_memory(&background).expect("背景应当是合法图片");
        assert_eq!((bot.width(), bot.height()), (256, 256));
        assert_eq!((admin.width(), admin.height()), (192, 192));
        assert_eq!((background.width(), background.height()), (1600, 900));
    }
}
