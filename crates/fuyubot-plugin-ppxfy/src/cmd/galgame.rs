//! `/p`（`/galgame`）：把 DMM / DLSite 作品页做成一条分享消息。
//!
//! 流程：抓页面 → 解析 → 建 Telegraph 页 → 逐项回复核对 → 记录入库 → 发成品消息。
//! 核对沿用「回复提示消息」那套交互，`/cancel` 随时可中断。
//!
//! 入库走 mongo-plugin 的 `storage.mongo` 服务（见 [`crate::db`]），按 title + brand
//! upsert 到 `ppxfy_galgame`：数据库不可用时只记日志，分享照常完成。

use std::path::PathBuf;
use std::sync::Arc;

use crate::core::page;
use crate::db;
use crate::galgame::{self, Info, Link, Site, Work};
use crate::host::messenger::{Piece, Sent};
use crate::host::reply::Wait;
use crate::host::task::Task;
use crate::host::{Ctx, Msg};
use crate::telegraph;

/// 每一步都带这句：开头的提示消息很快会被问题覆盖，靠它兜住 BACK / CANCEL。
const FOOTER: &str = "\n\n（BACK 回退 · CANCEL 取消）";

/// Telegram 的媒体说明上限：正文比它长就不能塞进照片的 caption。
const CAPTION_LIMIT: usize = 1024;

/// 核对步骤：`prompt` 生成当前问题，`apply` 把用户回复落到 [`Info`] 上。
struct Step {
    prompt: fn(&Info) -> String,
    apply: fn(&mut Info, &str),
}

/// 命令入口。出错只影响本次流程，不向外传播。
pub fn run(ctx: &Arc<Ctx>, msg: Msg, text: &str) {
    // 核对期间要能被 /cancel 打断；这里没有产物要清理，清理钩子留空。
    let task = ctx.tasks.register(msg.chat_id, Box::new(|| {}));
    if let Err(error) = flow(ctx, &task, msg, text) {
        ctx.error(&format!("ppxfy: galgame 流程失败: {error}"));
        ctx.messenger
            .try_send_text(msg.chat_id, &format!("❌ 流程中断: {error}"), Some(msg.id));
    }
    ctx.tasks.end(&task);
}

/// 主流程。返回 `Err` 只用于「需要告知用户的异常」，用户主动取消不算错误。
fn flow(ctx: &Ctx, task: &Task, msg: Msg, text: &str) -> Result<(), String> {
    let url = extract_url(text);
    let Some(site) = Site::detect(url) else {
        ctx.messenger.try_send_text(
            msg.chat_id,
            "请提供 DMM / DLSite 的作品链接：/p <链接>",
            Some(msg.id),
        );
        return Ok(());
    };

    let tips = ctx
        .messenger
        .send_text(
            msg.chat_id,
            &format!("🔍 正在解析 {} 页面…", site.name()),
            Some(msg.id),
        )
        .map_err(|error| format!("发送提示消息失败: {error}"))?;
    task.set_tips(tips.message_id);

    // DLSite 有区域限制，只有它需要走日本线路代理。
    let proxy = match site {
        Site::Dlsite => ctx.config.dlsite_proxy.as_deref(),
        Site::Dmm => None,
    };
    // 抓取失败就是「这条链接没戏」，改掉提示消息即可，不必再往聊天里丢一条错误。
    let fetched = match page::fetch(url, proxy) {
        Ok(fetched) => fetched,
        Err(error) => {
            ctx.messenger.edit_text(
                tips.chat_id,
                tips.message_id,
                &format!("❌ 抓取页面失败：{error}"),
            );
            return Ok(());
        }
    };
    let work = galgame::parse(site, &fetched.html, &fetched.url);
    if work.title.is_empty() {
        ctx.messenger.edit_text(
            tips.chat_id,
            tips.message_id,
            "❌ 页面解析失败：没读到作品标题，可能链接不对或页面改版了。",
        );
        return Ok(());
    }
    ctx.info(&format!(
        "ppxfy: 解析到《{}》品牌={}，{} 张预览图{}",
        work.title,
        work.brand,
        work.images.len(),
        if work.video.is_some() {
            "，含预览视频"
        } else {
            ""
        }
    ));

    // token 只从配置来（代码里不留凭据），没配就别去请求，直接说清楚缺哪一项。
    if ctx.config.telegraph.token.trim().is_empty() {
        ctx.error("ppxfy: 未配置 telegraph_token，无法创建 Telegraph 页面");
        ctx.messenger.edit_text(
            tips.chat_id,
            tips.message_id,
            "❌ 未配置 telegraph_token（[plugin.ppxfy] 段），已中止。",
        );
        return Ok(());
    }

    ctx.messenger
        .edit_text(tips.chat_id, tips.message_id, "🖼 正在创建 Telegraph 页面…");
    let Some(telegraph) = telegraph::create(&work, &ctx.config.telegraph) else {
        ctx.messenger.edit_text(
            tips.chat_id,
            tips.message_id,
            "❌ Telegraph 页面创建失败，已中止。",
        );
        return Ok(());
    };
    ctx.info(&format!("ppxfy: Telegraph 页面 {telegraph}"));

    interact(ctx, task, tips, draft(&work, telegraph, url), &work);
    Ok(())
}

/// 拿解析结果起一份待核对的数据：标题 / 类型 / 发售日 / 入正地址先填好，其余留空。
fn draft(work: &Work, telegraph: String, source: &str) -> Info {
    Info {
        title: work.title.clone(),
        release_date: work.release_date.clone(),
        genre: galgame::split_genres(&work.genre),
        buy: vec![Link {
            name: galgame::platform_name(source).to_owned(),
            link: source.to_owned(),
        }],
        telegraph,
        ..Info::default()
    }
}

/// 逐项核对，全部完成后发成品消息。
fn interact(ctx: &Ctx, task: &Task, tips: Sent, mut info: Info, work: &Work) {
    let mut index = 0;
    let mut back_hint = false;

    while index < STEPS.len() {
        let question = (STEPS[index].prompt)(&info);
        let prompt = if back_hint {
            format!("⚠️ 已经是第一步了，无法再回退。\n\n{question}{FOOTER}")
        } else {
            format!("{question}{FOOTER}")
        };
        back_hint = false;
        ctx.messenger
            .edit_text(tips.chat_id, tips.message_id, &prompt);

        let reply = match ctx.wait_reply(
            task,
            tips.chat_id,
            tips.message_id,
            ctx.config.listen_timeout,
        ) {
            Wait::Reply(reply) => reply,
            Wait::Timeout => {
                ctx.messenger.edit_text(
                    tips.chat_id,
                    tips.message_id,
                    "等待回复超时，已取消核对。",
                );
                return;
            }
            Wait::Cancelled => {
                ctx.messenger
                    .edit_text(tips.chat_id, tips.message_id, "🛑 已取消核对。");
                return;
            }
            Wait::Shutdown => return,
        };

        let reply = reply.trim();
        if reply.eq_ignore_ascii_case("cancel") {
            ctx.messenger
                .edit_text(tips.chat_id, tips.message_id, "🛑 已取消核对。");
            return;
        }
        if reply.eq_ignore_ascii_case("back") {
            if index > 0 {
                index -= 1;
            } else {
                // 第一步无法回退：下一轮在同一问题前加提示，不消耗用户额外输入。
                back_hint = true;
            }
            continue;
        }

        if !is_skip(reply) {
            (STEPS[index].apply)(&mut info, reply);
        }
        index += 1;
    }

    ctx.messenger
        .edit_text(tips.chat_id, tips.message_id, "已完成创建详细信息参数");
    ctx.info(&format!(
        "ppxfy: 核对完成 title={} telegraph={}",
        info.title, info.telegraph
    ));

    let pieces = galgame::share_text(&info);
    if let Err(error) = send_result(ctx, tips.chat_id, &info, &work.cover, &pieces) {
        ctx.error(&format!("ppxfy: 发送成品消息失败: {error}"));
    }

    // 入库放在成品消息之后：服务调用是同步的，数据库不可达时会等 server selection
    // 超时（默认 10s），不能让它拖住用户等成品消息。放到最后执行，
    // 入库失败不会影响分享。
    persist(ctx, work, &info);
}

/// 把核对结果写入 `ppxfy_galgame`。
///
/// 记录组装与入库规则：匹配键 title + brand，
/// 已存在则更新、不存在则插入。数据库只是留档，任何失败都只记日志。
fn persist(ctx: &Ctx, work: &Work, info: &Info) {
    // 服务探测只看方法表、不发 IO；没装 mongo-plugin 时不必再走一次必然失败的调用。
    if !db::available(ctx) {
        ctx.warn("ppxfy: storage.mongo 服务不可用（mongo-plugin 未安装或未启动），本次不入库");
        return;
    }

    match db::save(ctx, &db::Record::build(work, info)) {
        Ok(db::SaveOutcome::Inserted) => ctx.info(&format!(
            "ppxfy: 作品记录已新增 title={} brand={}",
            info.title, work.brand
        )),
        Ok(db::SaveOutcome::Updated) => ctx.info(&format!(
            "ppxfy: 作品记录已更新 title={} brand={}",
            info.title, work.brand
        )),
        Err(error) => ctx.warn(&format!(
            "ppxfy: 作品记录入库失败（不影响成品消息）: {error}"
        )),
    }
}

/// 发成品消息：正文按样式发（折叠引用块），封面按核对结果带剧透遮罩。
///
/// 正文能塞进媒体说明（[`CAPTION_LIMIT`]）就一条消息带图；塞不下则图先单独发、
/// 正文随后——否则 TDLib 会直接以「说明过长」拒绝整条消息，连文字都发不出去。
fn send_result(
    ctx: &Ctx,
    chat_id: i64,
    info: &Info,
    cover: &str,
    pieces: &[Piece],
) -> Result<(), String> {
    let length: usize = pieces
        .iter()
        .map(|piece| piece.text().chars().count())
        .sum();
    let path = (!cover.is_empty())
        .then(|| cover_file(ctx, chat_id, cover))
        .flatten();

    if let Some(path) = path {
        let caption = (length <= CAPTION_LIMIT).then_some(pieces);
        match ctx
            .messenger
            .send_photo(chat_id, &path, caption, info.blur_cover)
        {
            // 说明和正文一起发出去了，不用再发第二条。
            Ok(_) if caption.is_some() => {
                let _ = std::fs::remove_file(&path);
                return Ok(());
            }
            Ok(_) => {}
            Err(error) => ctx.warn(&format!("ppxfy: 封面发送失败，只发正文: {error}")),
        }
        let _ = std::fs::remove_file(&path);
    }

    ctx.messenger
        .send_styled(chat_id, pieces)
        .map(|_| ())
        .map_err(|error| format!("发送消息失败: {error}"))
}

/// 把封面下载成暂存文件（TDLib 只认本地文件）；失败返回 `None`。
fn cover_file(ctx: &Ctx, chat_id: i64, cover: &str) -> Option<PathBuf> {
    let bytes = match page::fetch_image(cover) {
        Ok(bytes) => bytes,
        Err(error) => {
            ctx.warn(&format!("ppxfy: 封面下载失败: {error}"));
            return None;
        }
    };
    // 一个聊天同一时间只有一条流程，按 chat 命名不会撞车。
    let path = ctx
        .config
        .cache_dir
        .join(format!("galgame-cover-{chat_id}.jpg"));
    if let Err(error) = std::fs::create_dir_all(&ctx.config.cache_dir) {
        ctx.warn(&format!("ppxfy: 创建缓存目录失败: {error}"));
        return None;
    }
    if let Err(error) = std::fs::write(&path, bytes) {
        ctx.warn(&format!("ppxfy: 写入封面失败: {error}"));
        return None;
    }
    Some(path)
}

/// 取消息里的第一个链接；没有链接时把整条文本当链接。
fn extract_url(text: &str) -> &str {
    let text = text.trim();
    let start = ["https://", "http://"]
        .iter()
        .filter_map(|scheme| text.find(scheme))
        .min();

    match start {
        Some(start) => {
            let rest = &text[start..];
            let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
            &rest[..end]
        }
        None => text,
    }
}

/// 回复是不是「跳过」。
fn is_skip(reply: &str) -> bool {
    matches!(reply.trim().to_ascii_lowercase().as_str(), "ok" | "/skip")
}

/// 平台关键词 → 平台数组；认不出返回 `None`。
fn parse_platform(reply: &str) -> Option<Vec<String>> {
    let text = reply.trim().to_ascii_lowercase();
    if text.contains("都有") || text.contains("都支持") || text == "both" {
        return Some(vec!["android".to_owned(), "pc".to_owned()]);
    }
    if text.contains("安卓") || text == "android" || text == "android 版" || text == "安卓版"
    {
        return Some(vec!["android".to_owned()]);
    }
    if matches!(text.as_str(), "pc" | "pc版" | "电脑" | "win" | "windows") {
        return Some(vec!["pc".to_owned()]);
    }
    None
}

/// 核对步骤表，顺序即是提问顺序。
static STEPS: [Step; 11] = [
    Step {
        prompt: |info| {
            format!(
                "当前解析到的游戏标题为：\n{}\n\n如需修改请直接回复新标题，回复 OK 表示无需修改。",
                info.title
            )
        },
        apply: |info, reply| info.title = reply.trim().to_owned(),
    },
    Step {
        prompt: |_| "请发送游戏中文名，回复 OK 表示直接使用原标题。".to_owned(),
        apply: |info, reply| info.chinese_name = reply.trim().to_owned(),
    },
    Step {
        prompt: |info| {
            format!(
                "当前游戏类型为：{}\n\n如需修改请直接回复新类型（多个用 /、, 分隔），回复 OK 表示无需修改。",
                if info.genre.is_empty() {
                    "无".to_owned()
                } else {
                    info.genre.join(" / ")
                }
            )
        },
        apply: |info, reply| {
            let genres = galgame::split_genres(reply);
            if !genres.is_empty() {
                info.genre = genres;
            }
        },
    },
    Step {
        prompt: |info| {
            format!(
                "当前入正地址为：\n{}\n\n如需修改请直接回复新链接（可多条，空格分隔），回复 OK 表示无需修改。",
                info.buy
                    .iter()
                    .map(|buy| buy.link.as_str())
                    .collect::<Vec<_>>()
                    .join("\n")
            )
        },
        apply: |info, reply| {
            let links: Vec<Link> = reply
                .split_whitespace()
                .map(|link| Link {
                    name: galgame::platform_name(link).to_owned(),
                    link: link.to_owned(),
                })
                .collect();
            if !links.is_empty() {
                info.buy = links;
            }
        },
    },
    Step {
        prompt: |_| {
            "请发送开发商信息，多个用英文逗号分隔，如：ゆずソフト, YUZUSOFT\n\n回复 OK 跳过。"
                .to_owned()
        },
        apply: |info, reply| info.developer = galgame::split_commas(reply),
    },
    Step {
        prompt: |_| {
            "请发送发行商信息，多个用英文逗号分隔，如：HIKARI FIELD, NekoNyan Ltd.\n\n回复 OK 跳过。"
                .to_owned()
        },
        apply: |info, reply| info.publisher = galgame::split_commas(reply),
    },
    Step {
        prompt: |_| "请发送游戏介绍信息\n\n回复 OK 跳过。".to_owned(),
        apply: |info, reply| info.description = reply.trim().to_owned(),
    },
    Step {
        prompt: |_| {
            "请发送其他信息（原画 / 剧本 / 声优 等补充说明），支持换行\n\n回复 OK 跳过。".to_owned()
        },
        apply: |info, reply| info.extra = reply.trim().to_owned(),
    },
    Step {
        prompt: |_| {
            "请选择该游戏支持的平台：\n- 回复「安卓」仅支持安卓\n- 回复「PC」仅支持 PC\n- 回复「都有」同时支持安卓与 PC\n\n回复 OK 跳过。"
                .to_owned()
        },
        apply: |info, reply| {
            if let Some(platform) = parse_platform(reply) {
                info.platform = platform;
            }
        },
    },
    Step {
        prompt: |_| {
            "请发送该游戏压缩包的 SHA256（打包完成消息中提供），用于后期校验压缩包是否被修改\n\n回复 OK 跳过。"
                .to_owned()
        },
        apply: |info, reply| info.zip_sha256 = reply.trim().to_owned(),
    },
    Step {
        prompt: |_| {
            "是否对封面图像开启剧透遮罩？（发送时默认有雪花遮挡，点击后可见，用于遵循 TG 对不良图像的遮挡规范）\n- 回复「是」开启遮罩\n- 回复「否」不遮罩\n\n回复 OK 默认不遮罩。"
                .to_owned()
        },
        apply: |info, reply| {
            info.blur_cover = matches!(
                reply.trim().to_ascii_lowercase().as_str(),
                "是" | "yes" | "y"
            );
            // 只有真正回答过才允许覆盖库里的 blurCover（跳过时保持 None）。
            info.blur_cover_answered = true;
        },
    },
];
