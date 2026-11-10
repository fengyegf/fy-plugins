//! `/musiccard` 的业务流程。
//!
//! 顺序固定：进度消息 → 取歌 → 取封面 → 选歌词 → 渲染 → 写临时文件 → 发照片；
//! 无论成败都要删临时文件、删进度消息，失败时再回一条原因。
//!
//! 清理用 [`Cleanup`]（Drop 守卫）实现：进度消息发出后立刻建守卫，之后无论是正常
//! 返回、提前 `?` 出错，还是 `panic!` unwind（netease / render 里炸了），守卫都会在
//! drop 时把临时文件与进度消息清掉。

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::host::messenger::Sent;
use crate::host::{Ctx, Msg};

/// 无参数时的用法提示。
const USAGE: &str = "请提供歌曲 ID、网易云歌曲链接或关键词。\n例如：/musiccard 347230";
/// 处理期间的进度消息。
const PROGRESS: &str = "正在生成音乐卡片，请稍候…";

/// 临时文件序号：同一进程内并发调用（连纳秒时间戳都撞上）也不会重名。
static SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Drop 守卫：正常返回与 panic unwind 走**同一条**清理路径。
///
/// 注意：`delete_messages` 会阻塞等 TDLib 响应，所以守卫只能在业务线程里 drop，
/// 不能出现在宿主回调线程上（本文件全部运行在业务线程）。
struct Cleanup<'a> {
    ctx: &'a Ctx,
    chat_id: i64,
    /// 进度消息 id；进度消息没发出去就是 `None`。
    progress_id: Option<i64>,
    /// 已落盘的临时文件；`generate` 成功写盘后登记。
    temp_path: Option<PathBuf>,
}

impl<'a> Cleanup<'a> {
    fn new(ctx: &'a Ctx, chat_id: i64, progress: Option<Sent>) -> Self {
        Self {
            ctx,
            chat_id,
            progress_id: progress.map(|sent| sent.message_id),
            temp_path: None,
        }
    }

    /// 登记一个已落盘的临时文件，drop 时删除。
    fn track(&mut self, path: PathBuf) {
        self.temp_path = Some(path);
    }
}

impl Drop for Cleanup<'_> {
    fn drop(&mut self) {
        // 先删本机文件（纯本地操作），再删远端消息（要等 TDLib 响应）。
        if let Some(path) = self.temp_path.take() {
            match std::fs::remove_file(&path) {
                Ok(()) => {}
                // 已经被别人删掉（或本来就没写成功）不算错误。
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => self.ctx.warn(&format!(
                    "musiccard: 删除临时文件 {} 失败: {error}",
                    path.display()
                )),
            }
        }
        if let Some(message_id) = self.progress_id.take() {
            self.ctx
                .messenger
                .delete_messages(self.chat_id, &[message_id]);
        }
    }
}

/// 命令入口。任何失败都只影响本次流程，不向外传播。
pub fn run(ctx: &Arc<Ctx>, msg: Msg, query: &str) {
    let query = query.trim();
    if query.is_empty() {
        ctx.messenger
            .try_send_text(msg.chat_id, USAGE, Some(msg.id));
        return;
    }

    let progress = ctx
        .messenger
        .try_send_text(msg.chat_id, PROGRESS, Some(msg.id));

    // 守卫必须在进度消息发出后**立刻**建：之后任何路径（含 panic unwind）都由它清理。
    let mut cleanup = Cleanup::new(ctx.as_ref(), msg.chat_id, progress);

    let outcome = generate(ctx, msg, query, &mut cleanup);
    // 显式 drop：先把临时文件与进度消息清掉，再回失败提示。
    drop(cleanup);

    if let Err(reason) = outcome {
        ctx.error(&format!(
            "musiccard: 生成失败（chat={}, query={query}）: {reason}",
            msg.chat_id
        ));
        ctx.messenger.try_send_text(
            msg.chat_id,
            &format!("生成音乐卡片失败：{reason}"),
            Some(msg.id),
        );
    }
}

/// 取数 → 渲染 → 落盘 → 上传。落盘路径登记到 `cleanup`，由调用方（守卫）清理。
fn generate(
    ctx: &Arc<Ctx>,
    msg: Msg,
    query: &str,
    cleanup: &mut Cleanup<'_>,
) -> Result<(), String> {
    // 1. 取歌（纯 ID / 链接 / 关键词由 netease 层判断）。
    let song = crate::netease::get_song(query).map_err(|error| error.to_string())?;

    // 2. 封面原图字节；解码交给渲染层。
    let cover = crate::netease::fetch_cover(&song.cover_url).map_err(|error| error.to_string())?;

    // 3. 选歌词并匹配译文。
    let lyrics = crate::netease::select_lyrics(&song.lyric, &song.translated_lyric);

    // 4. 版面上的文本一律先按**字符**截断。
    let title = crate::util::truncate(&song.name, 22);
    let artists = crate::util::truncate(&song.artist_line(), 28);
    let album = crate::util::truncate(&song.album, 24);

    // 日期按宿主本地时区：month 是 "Nov" 这种三字母英文缩写，day 两位补零。
    let today = chrono::Local::now();
    let month = today.format("%b").to_string();
    let day = today.format("%d").to_string();
    let qr_content = song.share_url();

    let card = crate::render::Card {
        title: &title,
        artists: &artists,
        album: &album,
        cover: cover.as_slice(),
        qr_content: &qr_content,
        month: &month,
        day: &day,
        lyrics: &lyrics,
    };
    let jpeg = crate::render::render_card(&card).map_err(|error| error.to_string())?;

    // 5. 落盘：TDLib 只能上传本地文件。名字带进程号 + 纳秒 + 序号，保证唯一。
    let dir = &ctx.config.temp_dir;
    std::fs::create_dir_all(dir)
        .map_err(|error| format!("创建临时目录 {} 失败：{error}", dir.display()))?;
    let path = temp_path(dir);
    std::fs::write(&path, &jpeg)
        .map_err(|error| format!("写入临时文件 {} 失败：{error}", path.display()))?;
    // 写盘成功就先登记：之后哪怕上传失败 / panic，守卫也会删掉它。
    cleanup.track(path.clone());

    // 6. 发照片，并回复触发命令的那条消息。
    ctx.messenger
        .send_photo(msg.chat_id, &path, Some(msg.id), None, false)
        .map_err(|error| format!("发送图片失败：{error}"))?;
    Ok(())
}

/// 生成一个临时 JPEG 路径（不创建文件）。
///
/// 唯一性来源：进程号 + 系统时间纳秒 + 进程内自增序号。同一毫秒内并发也不会撞。
fn temp_path(dir: &Path) -> PathBuf {
    let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or_default();
    dir.join(format!(
        "musiccard-{}-{nanos}-{sequence}.jpg",
        std::process::id()
    ))
}
