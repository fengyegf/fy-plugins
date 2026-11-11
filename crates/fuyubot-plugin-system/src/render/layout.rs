//! 版面几何：HTML 模板（[`super::template`]）与位图合成（[`super::compose`]）共用的一份坐标。
//!
//! 坐标全部是**画布坐标**（1400×800），单位 px，直接对应 `system.vue` 里按 4px 间距
//! 换算出来的 Tailwind 数值（`w-350`→1400、`h-200`→800、`w-45`→180、`p-4`→16、
//! `mt-10`→40……）。
//!
//! 共用一份常量是刻意的：头像的**白色圆环**画在 HTML 层里、**照片**由 `image` 贴在
//! 像素层上，两边只要差 1px 就会露出错位的白边。所以「外框 / 内圆」两组坐标只在
//! 这里定义一次，模板与合成各取所需。
//!
//! ```text
//! ┌ 1400×800 ────────────────────────────────────────────────────────────┐
//! │  (20,50) ┌──────┐  名字 36px / @username 24px / ID 24px              │
//! │          │ 180  │                                                   │
//! │          └──────┘                                                   │
//! │  ┌ (20,270) 1360×480 ──────────────────────────────────────────────┐ │
//! │  │ 左列 36..684                         │ 1px 竖线 700 │ 右列 716.. │ │
//! │  │ admin / TDLib / 使用情况              │              │ 磁盘 / 插件 │ │
//! │  └──────────────────────────────────────────────────────────────────┘ │
//! └──────────────────────────────────────────────────────────────────────┘
//! ```

use crate::render::{HEIGHT, WIDTH};

// ---------------------------------------------------------------------------
// 画布与内容列
// ---------------------------------------------------------------------------

/// 内容列（头像行 + 信息大卡片）的左边距。
pub const CONTENT_LEFT: u32 = 20;
/// 内容列宽度。
pub const CONTENT_WIDTH: u32 = 1360;
/// 内容列上下留白：头像行顶 50，大卡片底 750。
pub const CONTENT_TOP: u32 = 50;

// ---------------------------------------------------------------------------
// 头像行（`w-45 h-45` + `p-5` 文字列）
// ---------------------------------------------------------------------------

/// 机器人头像外框边长（180）。
pub const BOT_AVATAR_SIZE: u32 = 180;
/// 机器人头像外框左上角。
pub const BOT_AVATAR_LEFT: u32 = CONTENT_LEFT;
pub const BOT_AVATAR_TOP: u32 = CONTENT_TOP;
/// 头像白边宽度。
pub const AVATAR_BORDER: u32 = 5;
/// 机器人头像内圆边长（170，白边 5px 在外圈）。
pub const BOT_AVATAR_INNER_SIZE: u32 = BOT_AVATAR_SIZE - 2 * AVATAR_BORDER;
pub const BOT_AVATAR_INNER_LEFT: u32 = BOT_AVATAR_LEFT + AVATAR_BORDER;
pub const BOT_AVATAR_INNER_TOP: u32 = BOT_AVATAR_TOP + AVATAR_BORDER;

/// 头像行文字列左边距（`p-5` = 20px）。
pub const BOT_TEXT_LEFT: u32 = BOT_AVATAR_LEFT + BOT_AVATAR_SIZE + 20;
/// 文字列最大宽度（右边留 24px）。
pub const BOT_TEXT_WIDTH: u32 = WIDTH - BOT_TEXT_LEFT - 24;
/// 名字 36px / 行高 44px。
pub const BOT_NAME_FONT: u32 = 36;
pub const BOT_NAME_LINE_HEIGHT: u32 = 44;
/// 名字最小字号：超长名字先缩字号，缩到 24 还放不下才截断。
pub const BOT_NAME_MIN_FONT: u32 = 24;
/// `@username` 与 `ID:` 两行 24px / 行高 32px。
pub const BOT_SUB_FONT: u32 = 24;
pub const BOT_SUB_LINE_HEIGHT: u32 = 32;

/// 三行文字整体垂直居中的位置（180 − (44 + 32 + 32) = 72，上下各 36）。
pub const BOT_NAME_TOP: u32 = BOT_AVATAR_TOP + 36;
pub const BOT_USERNAME_TOP: u32 = BOT_NAME_TOP + BOT_NAME_LINE_HEIGHT;
pub const BOT_ID_TOP: u32 = BOT_USERNAME_TOP + BOT_SUB_LINE_HEIGHT;

// ---------------------------------------------------------------------------
// 信息大卡片（`w-340 h-120 rounded-xl mt-10`）
// ---------------------------------------------------------------------------

/// 大卡片左上角与尺寸（头像行 180 + 间隔 40 = 270）。
pub const CARD_LEFT: u32 = CONTENT_LEFT;
pub const CARD_TOP: u32 = CONTENT_TOP + BOT_AVATAR_SIZE + 40;
pub const CARD_WIDTH: u32 = CONTENT_WIDTH;
pub const CARD_HEIGHT: u32 = 480;
pub const CARD_RADIUS: u32 = 12;

/// 两列之间的 1px 竖线（`border-l border-white/30`）。
pub const DIVIDER_X: u32 = CARD_LEFT + CARD_WIDTH / 2;

/// 列内边距（`p-4`）。
pub const COLUMN_PADDING: u32 = 16;
/// 左 / 右两列的左边距与列宽（每列 680 − 32 = 648）。
pub const COL_LEFT_X: u32 = CARD_LEFT + COLUMN_PADDING;
pub const COL_RIGHT_X: u32 = DIVIDER_X + COLUMN_PADDING;
pub const COL_WIDTH: u32 = CARD_WIDTH / 2 - 2 * COLUMN_PADDING;

/// 面板（`bg-white/70 rounded-xl p-2`）的边框与内边距。
pub const PANEL_BORDER: u32 = 1;
pub const PANEL_PADDING: u32 = 8;
/// 面板边框 + 内边距的总占用（左右各一份，共 18）。
pub const PANEL_CHROME: u32 = 2 * (PANEL_BORDER + PANEL_PADDING);
/// 面板内可用宽度（630）。
pub const PANEL_INNER_WIDTH: u32 = COL_WIDTH - PANEL_CHROME;
/// 左右两列面板内的内容起点 x（45 / 725）。
pub const LEFT_INNER_X: u32 = COL_LEFT_X + PANEL_BORDER + PANEL_PADDING;
pub const RIGHT_INNER_X: u32 = COL_RIGHT_X + PANEL_BORDER + PANEL_PADDING;

/// 区块标题（`text-lg font-semibold`）：18px / 行高 28px。
pub const SECTION_TITLE_FONT: u32 = 18;
pub const SECTION_TITLE_HEIGHT: u32 = 28;
/// 标题与面板之间的间距（`mt-1` = 4px）。
pub const SECTION_TITLE_GAP: u32 = 4;
/// 相邻区块之间的间距（`mt-2` = 8px）。
pub const SECTION_GAP: u32 = 8;

/// 内容区顶端（卡片内边距之后）。
pub const CONTENT_INNER_TOP: u32 = CARD_TOP + COLUMN_PADDING;
/// 内容区底端：738 = 卡片底 750 − 内边距 16…… 这里取 734（卡片底 − 16）。
pub const CONTENT_INNER_BOTTOM: u32 = CARD_TOP + CARD_HEIGHT - COLUMN_PADDING;

/// 一行信息（18px / 行高 28px）。
pub const ROW_FONT: u32 = 18;
pub const ROW_HEIGHT: u32 = 28;

// ---------------------------------------------------------------------------
// 左列：admin
// ---------------------------------------------------------------------------

/// admin 区块标题顶部（第一个区块带 `mt-2`）。
pub const ADMIN_TITLE_TOP: u32 = CONTENT_INNER_TOP + SECTION_GAP;
/// admin 面板顶部。
pub const ADMIN_PANEL_TOP: u32 = ADMIN_TITLE_TOP + SECTION_TITLE_HEIGHT + SECTION_TITLE_GAP;

/// 管理员头像外框边长（`w-25 h-25` = 100）。
pub const ADMIN_AVATAR_SIZE: u32 = 100;
pub const ADMIN_AVATAR_INNER_SIZE: u32 = ADMIN_AVATAR_SIZE - 2 * AVATAR_BORDER;
/// 管理员头像外框左上角：面板内左上角。
pub const ADMIN_AVATAR_LEFT: u32 = LEFT_INNER_X;
pub const ADMIN_AVATAR_TOP: u32 = ADMIN_PANEL_TOP + PANEL_BORDER + PANEL_PADDING;
pub const ADMIN_AVATAR_INNER_LEFT: u32 = ADMIN_AVATAR_LEFT + AVATAR_BORDER;
pub const ADMIN_AVATAR_INNER_TOP: u32 = ADMIN_AVATAR_TOP + AVATAR_BORDER;

/// 管理员文字列左边距。
pub const ADMIN_TEXT_LEFT: u32 = ADMIN_AVATAR_LEFT + ADMIN_AVATAR_SIZE + 16;
/// 管理员文字列宽度。
pub const ADMIN_TEXT_WIDTH: u32 = LEFT_INNER_X + PANEL_INNER_WIDTH - ADMIN_TEXT_LEFT;
/// 姓名 24px 半粗 / 行高 32px。
pub const ADMIN_NAME_FONT: u32 = 24;
pub const ADMIN_NAME_LINE_HEIGHT: u32 = 32;
/// `@username` / `ID:` 18px / 行高 24px。
pub const ADMIN_SUB_FONT: u32 = 18;
pub const ADMIN_SUB_LINE_HEIGHT: u32 = 24;
/// 三行文字在 100px 头像行里垂直居中的起点（100 − 80 = 20，上 10）。
pub const ADMIN_NAME_TOP: u32 = ADMIN_AVATAR_TOP + 10;
pub const ADMIN_USERNAME_TOP: u32 = ADMIN_NAME_TOP + ADMIN_NAME_LINE_HEIGHT;
pub const ADMIN_ID_TOP: u32 = ADMIN_USERNAME_TOP + ADMIN_SUB_LINE_HEIGHT;

/// 底部小字（`text-xs`）：12px / 行高 16px，与头像行之间 4px。
pub const ADMIN_NOTE_FONT: u32 = 12;
pub const ADMIN_NOTE_LINE_HEIGHT: u32 = 16;
pub const ADMIN_NOTE_TOP: u32 = ADMIN_AVATAR_TOP + ADMIN_AVATAR_SIZE + 4;
/// admin 面板高度：边框 2 + 内边距 16 + 头像行 100 + 间隙 4 + 小字 16。
pub const ADMIN_PANEL_HEIGHT: u32 = PANEL_CHROME + ADMIN_AVATAR_SIZE + 4 + ADMIN_NOTE_LINE_HEIGHT;

// ---------------------------------------------------------------------------
// 左列：TDLib
// ---------------------------------------------------------------------------

pub const TDLIB_TITLE_TOP: u32 = ADMIN_PANEL_TOP + ADMIN_PANEL_HEIGHT + SECTION_GAP;
pub const TDLIB_PANEL_TOP: u32 = TDLIB_TITLE_TOP + SECTION_TITLE_HEIGHT + SECTION_TITLE_GAP;
/// 两行 18px 文本。
pub const TDLIB_PANEL_HEIGHT: u32 = PANEL_CHROME + 2 * ROW_HEIGHT;

// ---------------------------------------------------------------------------
// 左列：使用情况（CPU / RAM）
// ---------------------------------------------------------------------------

pub const GAUGE_TITLE_TOP: u32 = TDLIB_PANEL_TOP + TDLIB_PANEL_HEIGHT + SECTION_GAP;
pub const GAUGE_PANEL_TOP: u32 = GAUGE_TITLE_TOP + SECTION_TITLE_HEIGHT + SECTION_TITLE_GAP;

/// 使用率进度条（`h-3` = 12px，`rounded-full`）。
pub const BAR_HEIGHT: u32 = 12;
pub const BAR_RADIUS: u32 = 6;
/// 进度条与文字行垂直居中的偏移。
pub const BAR_OFFSET_Y: u32 = (ROW_HEIGHT - BAR_HEIGHT) / 2;
/// 使用情况一行：标签列宽、右侧百分比列宽、两侧间距。
pub const GAUGE_LABEL_WIDTH: u32 = 56;
pub const GAUGE_VALUE_WIDTH: u32 = 56;
pub const GAUGE_COLUMN_GAP: u32 = 12;

// ---------------------------------------------------------------------------
// 右列：磁盘空间
// ---------------------------------------------------------------------------

/// 磁盘区块标题与左列标题顶部对齐（右列首个区块没有额外的 `mt-2`，这里对齐视觉）。
pub const DISK_TITLE_TOP: u32 = CONTENT_INNER_TOP + SECTION_GAP;
pub const DISK_PANEL_TOP: u32 = DISK_TITLE_TOP + SECTION_TITLE_HEIGHT + SECTION_TITLE_GAP;

/// 磁盘行：标签列（「磁盘C」/「内部存储」/挂载点）与右侧 `used/total` 列。
///
/// 非 Windows 平台的标签是挂载点（`/data`），比单个盘符长，所以标签列给到 96。
pub const DISK_LABEL_WIDTH: u32 = 96;
pub const DISK_VALUE_WIDTH: u32 = 200;
pub const DISK_COLUMN_GAP: u32 = 12;

// ---------------------------------------------------------------------------
// 右列：已加载插件
// ---------------------------------------------------------------------------

/// 磁盘面板与插件标题之间的间距（`mt-3` = 12px）。
pub const PLUGIN_SECTION_GAP: u32 = 12;

/// 胶囊：14px 字、白底 80%、圆角 4、边框 1、内边距 8/4。
pub const CHIP_FONT: u32 = 14;
pub const CHIP_HEIGHT: u32 = 24;
pub const CHIP_RADIUS: u32 = 4;
pub const CHIP_PADDING_X: u32 = 8;
pub const CHIP_GAP: u32 = 5;
/// 名字与状态之间的间距。
pub const CHIP_INNER_GAP: u32 = 4;
/// 胶囊行数的硬上限：防止异常数据把面板撑出画布（正常由 [`max_chip_rows`] 决定）。
pub const MAX_CHIP_ROWS: u32 = 12;

/// 磁盘面板高度：边框 2 + 内边距 16 + `rows × 28`。
pub fn disk_panel_height(rows: u32) -> u32 {
    PANEL_CHROME + rows * ROW_HEIGHT
}

/// 插件区块标题顶部；`disk_rows` 是实际画出来的磁盘行数（1–4）。
pub fn plugin_title_top(disk_rows: u32) -> u32 {
    DISK_PANEL_TOP + disk_panel_height(disk_rows) + PLUGIN_SECTION_GAP
}

/// 插件面板顶部。
pub fn plugin_panel_top(disk_rows: u32) -> u32 {
    plugin_title_top(disk_rows) + SECTION_TITLE_HEIGHT + SECTION_TITLE_GAP
}

/// 胶囊区与底部统计行之间的间距。
pub const PLUGIN_FOOTER_GAP: u32 = 8;
/// 底部统计行高度（12px / 16px）。
pub const PLUGIN_FOOTER_HEIGHT: u32 = 16;

/// 插件面板高度：**按内容自适应**（不再铺满右列剩余空间）。
///
/// `chip_rows` 是实际排出来的胶囊行数（1–[`MAX_CHIP_ROWS`]）；插件为空时也传 1，
/// 占位行按 [`ROW_HEIGHT`] 算（比胶囊高 4px，取两者较大值兜住）。
pub fn plugin_panel_height(chip_rows: u32) -> u32 {
    let rows = chip_rows.max(1);
    let chips = rows * CHIP_HEIGHT + (rows - 1) * CHIP_GAP;
    PANEL_CHROME + chips.max(ROW_HEIGHT) + PLUGIN_FOOTER_GAP + PLUGIN_FOOTER_HEIGHT
}

/// 在给定磁盘行数下，右列还能放下几行插件胶囊。
///
/// 卡片高度是固定的，插件面板底端不能越过内容区底端；剩下的高度除了胶囊本身，
/// 还要扣掉面板边框 / 内边距与底部统计行。行数决定后，放不下的插件才折成 `+N`。
pub fn max_chip_rows(disk_rows: u32) -> u32 {
    let available = CONTENT_INNER_BOTTOM
        .saturating_sub(plugin_panel_top(disk_rows))
        .saturating_sub(PANEL_CHROME + PLUGIN_FOOTER_GAP + PLUGIN_FOOTER_HEIGHT);
    let rows = (available + CHIP_GAP) / (CHIP_HEIGHT + CHIP_GAP);
    rows.clamp(1, MAX_CHIP_ROWS)
}

/// 画布高度兜底校验用：右列最低点不应越过内容区底端。
pub const CANVAS_HEIGHT: u32 = HEIGHT;

#[cfg(test)]
mod tests {
    use super::*;

    /// 版面常量之间的基本一致性：不重叠、不溢出。
    #[test]
    fn geometry_is_sane() {
        // 头像行 + 间隔 + 大卡片正好落在内容列里。
        assert_eq!(BOT_AVATAR_TOP + BOT_AVATAR_SIZE + 40, CARD_TOP);
        assert_eq!(CARD_TOP + CARD_HEIGHT, 750);
        assert!(CARD_TOP + CARD_HEIGHT <= 800);

        // 两列不重叠，分隔线在两列中间。
        assert_eq!(DIVIDER_X, CARD_LEFT + CARD_WIDTH / 2);
        assert!(COL_LEFT_X + COL_WIDTH < DIVIDER_X);
        assert!(COL_RIGHT_X > DIVIDER_X);
        assert_eq!(
            COL_RIGHT_X + COL_WIDTH,
            CARD_LEFT + CARD_WIDTH - COLUMN_PADDING
        );

        // 左列三块依次向下，最后一块不越过内容区底端。
        assert!(ADMIN_PANEL_TOP > ADMIN_TITLE_TOP);
        assert!(TDLIB_PANEL_TOP > ADMIN_PANEL_TOP + ADMIN_PANEL_HEIGHT);
        assert!(GAUGE_PANEL_TOP > TDLIB_PANEL_TOP + TDLIB_PANEL_HEIGHT);
        let gauge_bottom = GAUGE_PANEL_TOP + PANEL_CHROME + 2 * ROW_HEIGHT;
        assert!(
            gauge_bottom <= CONTENT_INNER_BOTTOM,
            "CPU / RAM 两条 gauge 应当放得下"
        );

        // 右列：插件面板按内容自适应；可用行数由磁盘行数决定，且永远不越过底端。
        for rows in 1..=4 {
            let chip_rows = max_chip_rows(rows);
            assert!(chip_rows >= 2, "至少能放两行胶囊（实际 {chip_rows} 行）");
            let bottom = plugin_panel_top(rows) + plugin_panel_height(chip_rows);
            assert!(
                bottom <= CONTENT_INNER_BOTTOM,
                "{rows} 块磁盘 + {chip_rows} 行胶囊越过了内容区底端（{bottom} > {CONTENT_INNER_BOTTOM}）"
            );
        }
        assert!(
            max_chip_rows(1) > max_chip_rows(4),
            "磁盘越多，留给插件胶囊的行数越少"
        );
        assert!(
            max_chip_rows(4) >= 5,
            "四块磁盘时也要能放下常见数量（5 个以上）的插件"
        );
        assert!(
            plugin_panel_height(max_chip_rows(1)) > plugin_panel_height(2),
            "插件面板必须随胶囊行数变高，而不是固定高度"
        );

        // 头像内圆在外框里，且白边宽度正确。
        assert_eq!(BOT_AVATAR_INNER_LEFT, BOT_AVATAR_LEFT + AVATAR_BORDER);
        assert_eq!(
            BOT_AVATAR_INNER_LEFT + BOT_AVATAR_INNER_SIZE + AVATAR_BORDER,
            BOT_AVATAR_LEFT + BOT_AVATAR_SIZE
        );
        assert!(
            ADMIN_AVATAR_INNER_TOP + ADMIN_AVATAR_INNER_SIZE < ADMIN_PANEL_TOP + ADMIN_PANEL_HEIGHT
        );
    }
}
