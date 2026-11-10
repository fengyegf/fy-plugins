//! 打包并上传：问包名 → 7z 存储模式打包（超阈值自动分卷）→ 分卷上传 → 即时清理。
//!
//! 清理时机（服务器磁盘有限，能删就立刻删）：
//!
//! 1. 打包成功后立刻删掉被打包的目录（内容已经在 zip 里了）；
//! 2. 每批分卷上传成功后立刻删掉这些文件；
//! 3. 全部成功后再扫一遍残留，并清空 `Output/` 根目录。

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use sha2::{Digest, Sha256};

use crate::host::Ctx;
use crate::host::messenger::{Piece, Sent};
use crate::host::reply::Wait;
use crate::host::task::Task;
use crate::util::{fs as fsutil, seven};

/// 删除被占用文件的等待节奏。
const REMOVE_RETRY_DELAY: Duration = Duration::from_secs(1);

/// 相册（媒体组）单批上限：TDLib 的 `sendMessageAlbum` 只接受 2–10 条。
const ALBUM_SIZE: usize = 10;

/// 打包入口。`source` 是目录树里被选中的目录。
pub fn run(ctx: &Arc<Ctx>, task: &Arc<Task>, tips: Sent, source: &Path) -> Result<(), String> {
    let default_name = fsutil::sanitize_filename(
        &source
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "package".to_owned()),
    );

    // 1. 包名：`#` / 空 / 超时都用文件夹名，CANCEL 直接退出。
    ctx.messenger.edit_text(
        tips.chat_id,
        tips.message_id,
        &format!(
            "进入打包流程：{default_name}\n请回复压缩包名称（不含后缀），回复 # 使用文件夹名「{default_name}」，回复 CANCEL 取消；等待超时将默认使用文件夹名。"
        ),
    );
    let zip_name = match ctx.wait_reply(
        task,
        tips.chat_id,
        tips.message_id,
        ctx.config.listen_timeout,
    ) {
        Wait::Reply(text) => {
            let text = text.trim();
            if text.eq_ignore_ascii_case("CANCEL") {
                ctx.messenger
                    .edit_text(tips.chat_id, tips.message_id, "已取消打包流程。");
                return Ok(());
            }
            if text.is_empty() || text == "#" {
                default_name.clone()
            } else {
                fsutil::sanitize_filename(text)
            }
        }
        Wait::Timeout => {
            ctx.info(&format!("ppxfy: 等待包名超时，使用文件夹名 {default_name}"));
            default_name.clone()
        }
        Wait::Cancelled => {
            ctx.messenger
                .edit_text(tips.chat_id, tips.message_id, "已取消打包流程。");
            return Ok(());
        }
        Wait::Shutdown => return Ok(()),
    };

    // 2. 打包。
    ctx.messenger.edit_text(
        tips.chat_id,
        tips.message_id,
        &format!(
            "正在打包为 {zip_name}.zip（超过 {} MB 自动分卷），请稍等...",
            ctx.config.split_threshold_mb
        ),
    );
    let (files, sha256) = make_zip(ctx, source, &zip_name)?;

    // 3. 打包成功，立刻释放被打了包的目录。
    let report = fsutil::remove_with_retry(&[source.to_path_buf()], 3, REMOVE_RETRY_DELAY);
    if report.failed.is_empty() {
        ctx.info(&format!("ppxfy: 打包成功，已移除目录 {}", source.display()));
    } else {
        ctx.warn(&format!(
            "ppxfy: 打包成功但目录清理失败（{} 项残留）: {}",
            report.failed.len(),
            source.display()
        ));
    }

    // 4. 分卷上传，逐批清理。
    ctx.messenger.edit_text(
        tips.chat_id,
        tips.message_id,
        &format!("打包完成，正在上传 {} 个文件...", files.len()),
    );

    // 单文件直接发，多文件用相册（≤ ALBUM_SIZE 个一组）：相册一次请求传完整组，
    // 比逐条发省一半以上的往返；某一批失败只影响这一批，已成功的照常删盘。
    let total_files = files.len();
    let mut uploaded = 0usize;
    let mut failed = 0usize;

    for (batch_index, batch) in files.chunks(ALBUM_SIZE).enumerate() {
        if task.is_cancelled() || ctx.is_shutdown() {
            ctx.warn("ppxfy: 上传被取消");
            break;
        }

        let first = batch_index * ALBUM_SIZE + 1;
        let last = first + batch.len() - 1;
        let names: Vec<String> = batch.iter().map(|path| file_label(path)).collect();
        ctx.info(&format!(
            "ppxfy: 上传第 {} 批 {first}-{last}/{total_files}: {}",
            batch_index + 1,
            names.join(", ")
        ));

        // 进度里不写文件名：裸文本中的 `xxx.zip` 会被识别成链接（`.zip` 是真实 TLD）刷成蓝字，
        // 用 code 实体压住又会变成等宽可复制；文件名在上传出来的消息里本来就看得到。
        let progress = if batch.len() == 1 {
            format!("正在上传 {first}/{total_files}")
        } else {
            format!(
                "正在上传第 {} 批（{first}-{last}/{total_files}），共 {} 个文件",
                batch_index + 1,
                batch.len()
            )
        };
        ctx.messenger
            .edit_text(tips.chat_id, tips.message_id, &progress);

        let outcome = if batch.len() == 1 {
            ctx.messenger
                .send_document(tips.chat_id, &batch[0])
                .map(|_| ())
        } else {
            ctx.messenger.send_album(tips.chat_id, batch)
        };

        match outcome {
            Ok(()) => {
                uploaded += batch.len();
                remove_uploaded(ctx, batch);
            }
            Err(error) => {
                failed += batch.len();
                ctx.error(&format!(
                    "ppxfy: 上传失败，文件已保留待重试 {}: {error}",
                    names.join(", ")
                ));
            }
        }
    }

    // 5. 收尾：成功的分卷再扫一遍（上一次删除可能被占用），失败的原样留着。
    if failed == 0 {
        let leftovers: Vec<PathBuf> = files.iter().filter(|path| path.exists()).cloned().collect();
        if !leftovers.is_empty() {
            remove_uploaded(ctx, &leftovers);
        }
    }

    // 解压产物按设计在打包成功后一律清空（不管上传成败，内容已经在 zip 里了）。
    let output_dir = ctx.config.cache_dir.join(fsutil::OUTPUT_DIR);
    let report = fsutil::clear_dir(&output_dir);
    if report.failed.is_empty() {
        ctx.info(&format!(
            "ppxfy: 已清空解压目录 {}（{} 项）",
            output_dir.display(),
            report.removed.len()
        ));
    } else {
        for (path, error) in &report.failed {
            ctx.warn(&format!(
                "ppxfy: 清理解压产物失败 {}: {error}",
                path.display()
            ));
        }
    }

    let fail_note = if failed > 0 {
        format!("，失败 {failed} 个（文件保留在 cache/ZipOutput）")
    } else {
        String::new()
    };
    // SHA256 做成行内代码：Telegram 里点一下就能整段复制（反引号只是 Markdown 记号，
    // 新框架不解析 Markdown，直接写反引号会原样显示，所以要自己给 code 实体）。
    let summary =
        format!("打包完成，共 {total_files} 个文件，成功上传 {uploaded}{fail_note}\nSHA256: ");
    ctx.messenger.try_send_rich(
        tips.chat_id,
        &[Piece::plain(summary), Piece::code(sha256)],
        None,
    );
    Ok(())
}

/// 删除上传完成的文件；被占用时重试，仍失败就明确记一条日志（不再静默）。
fn remove_uploaded(ctx: &Ctx, paths: &[PathBuf]) {
    let report = fsutil::remove_with_retry(paths, 6, REMOVE_RETRY_DELAY);
    if report.failed.is_empty() {
        return;
    }
    for (path, error) in &report.failed {
        ctx.warn(&format!(
            "ppxfy: 上传成功但删除失败（磁盘可能仍被占用）{}: {error}",
            path.display()
        ));
    }
}

/// 取文件名用于提示。
fn file_label(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// 用 7z 以「存储」模式打包（`-mx=0`，不压缩、速度快），超过阈值自动分卷。
///
/// 以源目录为工作目录、用 `*` 存储其内容，避免解压后多出一层同名根目录。
/// 返回分卷列表（已排序）与第一卷的 SHA256。
fn make_zip(ctx: &Ctx, source: &Path, name: &str) -> Result<(Vec<PathBuf>, String), String> {
    let output_dir = ctx.config.cache_dir.join(fsutil::ZIP_OUTPUT_DIR);
    fs::create_dir_all(&output_dir).map_err(|error| format!("创建输出目录失败: {error}"))?;
    let base = output_dir.join(name);

    let is_dir = source.is_dir();
    let cwd = if is_dir {
        source.to_path_buf()
    } else {
        source
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."))
    };
    let store = if is_dir {
        "*".to_owned()
    } else {
        source
            .file_name()
            .map(|file| file.to_string_lossy().into_owned())
            .unwrap_or_else(|| "*".to_owned())
    };

    // 上一次同名打包可能留下分卷，先清掉：否则旧卷会被一起上传，也可能残留脏数据。
    let prefix = format!("{name}.zip");
    let stale: Vec<PathBuf> = fs::read_dir(&output_dir)
        .map_err(|error| format!("读取输出目录失败: {error}"))?
        .flatten()
        .filter(|entry| entry.file_name().to_string_lossy().starts_with(&prefix))
        .map(|entry| entry.path())
        .collect();
    fsutil::remove_all(&stale);

    let size_mb = fsutil::size_of(source) as f64 / 1024.0 / 1024.0;
    let mut args = vec![
        "a".to_owned(),
        format!("{}.zip", base.display()),
        store,
        "-mx=0".to_owned(),
        "-y".to_owned(),
    ];
    if size_mb > ctx.config.split_threshold_mb as f64 {
        args.push(format!("-v{}m", ctx.config.split_threshold_mb));
    }
    ctx.info(&format!(
        "ppxfy: 打包 {}（{size_mb:.2} MB）-> {}.zip",
        source.display(),
        base.display()
    ));

    let output = seven::run(
        &ctx.config.seven_zip,
        &args,
        Some(&cwd),
        ctx.config.unzip_timeout,
    )
    .map_err(|error| format!("打包失败: {error}"))?;
    if output.code != 0 {
        return Err(format!("打包失败: {}", seven::detail(&output.log)));
    }

    let mut files: Vec<PathBuf> = fs::read_dir(&output_dir)
        .map_err(|error| format!("读取输出目录失败: {error}"))?
        .flatten()
        .filter(|entry| entry.file_name().to_string_lossy().starts_with(&prefix))
        .map(|entry| entry.path())
        .collect();
    files.sort();

    let first = files
        .first()
        .ok_or_else(|| "未找到生成的压缩文件".to_owned())?;
    let sha256 = sha256_of(first)?;
    ctx.info(&format!(
        "ppxfy: 打包完成，{} 个文件，SHA256 {}…",
        files.len(),
        &sha256[..16.min(sha256.len())]
    ));
    Ok((files, sha256))
}

/// 计算文件的 SHA256（十六进制小写）。
fn sha256_of(path: &Path) -> Result<String, String> {
    let mut file = fs::File::open(path).map_err(|error| format!("打开文件失败: {error}"))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| format!("读取文件失败: {error}"))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}
