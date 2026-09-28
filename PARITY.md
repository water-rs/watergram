# PARITY — Watergram vs Telegram Desktop

功能矩阵，以官方 Telegram Desktop 为基准。状态：✅ 已实现 / 🟡 部分 / ❌ 未实现 / 🚫 不计划（TDLib 不支持的桌面端功能）。
测试名列出覆盖该功能的 `#[waterui::test]` 语义测试（`src/lib.rs` tests 模块）或 `tests/` 下的用例；`—` 表示尚无测试。

## 1. 认证与账号

| 功能 | 状态 | TDLib API | 测试 |
|---|---|---|---|
| api_id/api_hash 录入 + 校验 | ✅ | `setTdlibParameters` | `api_credentials_form`, `api_form_accepts_input` |
| Test DC 切换 | ✅ | `setTdlibParameters(use_test_dc)` | — |
| 手机号登录 | ✅ | `setAuthenticationPhoneNumber` | `phone_form` |
| 验证码 | ✅ | `checkAuthenticationCode` | `code_form_shows_phone` |
| 两步验证密码 | ✅ | `checkAuthenticationPassword` | — |
| 新用户注册 | ✅ | `registerUser` | — |
| QR 登录 | ✅ | `requestQrCodeAuthentication` + `authorizationStateWaitOtherDeviceConfirmation` | — |
| 登出 | ✅ | `logOut` | — |
| 多账号切换 | ✅ | 每账号独立 TDLib client + `db_<id>`/`files_<id>` + `getAuthorizationState` 切换 + `getMe` 标签 + `accounts.json` 持久化（非活动账号更新被丢弃，切回重查） | `account_switcher_opens` |
| 邮件验证（新注册要求的邮箱码） | 🚫 | `checkAuthenticationEmailCode`（UI 仅提示不支持） | — |

## 2. 聊天列表

| 功能 | 状态 | TDLib API | 测试 |
|---|---|---|---|
| 列表排序（位置/时间） | ✅ | `loadChats` + `updateChatPosition`/`updateChatLastMessage` | `chat_list_rows` |
| 未读数 / 提及徽标 | ✅ | `Chat.unread_count`/`unread_mention_count` → unread pill + `@` mention pill | `chat_list_rows`, `chat_row_mention_badge` |
| 置顶聊天 | ✅ | `toggleChatIsPinned` | — |
| 静音 / 通知设置 | ✅ | `setChatNotificationSettings` | — |
| 头像 + 在线状态点 | ✅ | `chat.photo`/`user.status` + `downloadFile`；首字母圆按 TDLib `accent_color_id` %7 上七色用户色板（无 accent 时按显示名哈希取色，跨聊天列表/消息头像/成员行一致） | `chat_list_rows` |
| typing/… 指示 | ✅ | `sendChatAction` + `updateChatAction` | — |
| 草稿显示 | ✅ | `updateChatDraftMessage`；列表行内红色 "Draft:" 前缀（r19 起，typing 优先） | — |
| 归档（Archive 文件夹） | ✅ | `addChatToList(Main/Archive)` + `ChatPosition` | `archive_toggle_rebuilds_list` |
| 聊天文件夹（Chat Folders） | ✅ | `updateChatFolders` + `ChatList::Folder`/`loadChats` 侧栏 tab；`createChatFolder`/`editChatFolder`/`deleteChatFolder`（名称+联系人群组频道开关；不含逐聊包含/排除编辑） | `folder_tabs_render`、`folder_editor_opens` |
| 已读回执标记 | ✅ | `viewMessages` + `UpdateChatReadOutbox` → ✓/✓✓ | `read_receipt_double_check` |
| 已标记为未读 | ✅ | `toggleChatIsMarkedAsUnread` + `UpdateChatIsMarkedAsUnread` | `marked_unread_shows_dot` |
| 自动删除定时器 | ✅ | `setChatMessageAutoDeleteTime` — 行菜单嵌套 `Menu`「Auto-delete ›」Off/1 day/1 week/1 month，`auto_delete` 秒级存于行态，标题行右侧 `clock_outline` 图标 + a11y 标签；demo/实机三宽实派 | `auto_delete_sets_and_clears` + 实测截图（r39_submenu*/r39_autodel_applied*） |
| 列表键盘导航 | ✅ | Up/Down 逐行移动、Home/End 跳首末、Enter 激活 — 框架 `navigate_list_row` 写 `list_selection` → `select_chat`（r34） | `keyboard_arrows_chat_list` |

## 3. 会话与消息

| 功能 | 状态 | TDLib API | 测试 |
|---|---|---|---|
| 打开会话 + 历史分页 | ✅ | `getChatHistory` 向前翻页 | `messages_render` |
| 发送文本 | ✅ | `sendMessage(inputMessageText)` | `composer_sends_and_clears` |
| 输入框 Markdown（*粗* _斜_ `码` ~~删~~ \|\|剧透\|\|） | ✅ | `parse_markdown` 发送时转 `FormattedText` 实体（UTF-16 offset），demo 走本地回显行；编辑路径同样解析 | `parse_markdown_strips_delimiters_and_offsets_utf16` `send_demo_echo_appends_outgoing_row` |
| 全局快捷键（Ctrl+F 搜索 / Ctrl+W 关窗 / Alt+↑↓ 切会话 等） | ❌ | 框架双层缺口：`Command::shortcut` 元数据存在（waterui menu.rs:261）但 hydrolysis 无任何消费点；修饰键 chord 在 hit_test.rs:2148 直接 return false，且 Event 枚举只有 Hover* —— 无键事件派发面。DOGFOOD r36-1 | — |
| 编辑消息 | ✅ | `editMessageText`；`message.edit_date` → 气泡内 "edited" 标记 | — |
| 删除消息 | ✅ | `deleteMessages` + 删除确认卡（"Delete N messages?"，私聊含 "Also delete for <peer>" 勾选框驱动 `revoke` 标志，Cancel/红色 Delete，Esc/点遮罩关闭） | `delete_confirm_card_flow` |
| 转发 | ✅ | `forwardMessages` | — |
| 回复 / 引用（含点击引用跳转原消息） | ✅ | `inputMessageReplyTo` + `jump_to_message`（本地命中直接高亮+滚动，未加载走 `getMessage`+历史） | `reply_banner_shows` / `reply_quote_jumps_to_loaded_message` |
| 复制文本 | ✅ | `getMessage` + 剪贴板；多选模式下按消息顺序 `\n` 拼接（r36） | `copy_selected_joins_in_message_order` |
| 已读标记（拉取后回执） | ✅ | `viewMessages` | — |
| 消息内搜索 | ✅ | `searchChatMessages` + 本地窗口全量命中高亮（`search_hit`→SelectionContainer+SelectionForeground span 配对，深浅气泡上均可读，r36 修正）+ n/N 计数 + ^/v 逐条跳转 + 结果行列表（防抖移除：nami debounce 在 hydrolysis 不发射，DOGFOOD r35-4/hydrolysis#228 workaround，直接监听 binding） | `chat_search_panel_opens` `chat_search_marks_and_clears` `chat_search_next_prev_cycle` |
| 跳转到某条消息（日期/回复定位） | ✅ | `getChatHistory` 窗口加载 + `List` `ScrollController<usize>` 按索引精确滚动 + 气泡高亮 | — |
| 置顶消息条 | ✅ | `getChatPinnedMessage` + `searchChatMessages(Pinned)` 全量列表；单条点击直接跳转，多条弹出列表逐条跳转（`VStack::for_each` 弹出层）+ `·N` 计数；`pinChatMessage`/`unpinChatMessage` | `pinned_banner_shows` `pinned_popup_opens_and_jumps` `pinned_tap_single_jumps_directly` |
| 服务消息（置顶/入群/拉人，居中灰条） | ✅ | `MessagePinMessage`/`MessageChatAddMembers`/`MessageChatJoinBy*` → 居中 muted pill（"You"/发送者名前缀，拉人解析成员名）；独立 List 行，`ListItem::insets`+`.list_min_row_height(0)` 给 Desktop 间距（34pt 实测，waterui#1252 已落地）；不参与消息组、不可多选、无右键菜单；demo pin 路径同步追加服务行 | `pin_appends_service_row` / `service_row_not_selectable` / `service_rows_break_runs` |
| Reactions 展示（统一 pill 组件：气泡内气泡色派生 tint、已选 accent） | ✅ | `UpdateMessageInteractionInfo`（数量+自己的选择） | `reactions_render` |
| Reactions 发送 | ✅ | `addMessageReaction`/`removeMessageReaction`（右键菜单 👍❤️😂😮😢 + 取消） | — |
| 发送中/失败状态 | ✅ | `updateMessageSendSucceeded`/`Failed` 行内图标；点 ✗ → `resendMessages` | `resend_failed_marks_pending` |
| 定时消息 / 静默发送 | ✅ | `sendMessage(MessageSendOptions{disable_notification,scheduling_state=SendAtDate})` — 发送按钮右键 | — (send_opt 路径） |
| 链接预览 | ✅ | `MessageText.link_preview` → Desktop 式预览卡片：accent 竖条 + site·title·desc（外发气泡内 AccentForeground/内 Accent）；点卡片经 `robius_open` 打开浏览器（`link_opened` 可观测） | `link_preview_line_shows` `link_preview_card_shows_site_title_desc` `link_card_opens_url` |
| 剧透点按揭示 | ✅ | Spoiler 实体 → 遮掩态（masked 文本前景=气泡填充融入）+ 点按揭示（`revealed_spoilers` 切换 `.visible`，a11y Button「Hidden text — tap to reveal」揭示后隐藏） | `spoiler_tap_reveals` `spoiler_tap_reveals_offscreen` |
| 消息翻译 | ✅ | `translateMessageText(to="en")` → `translated` map，气泡内联替换为译文 + "Translated to English — show original" caption 点按还原（`.visible` 对换）；右键菜单仅对非空入站消息显示 | `translate_swaps_and_restores` + 实测截图（r39_translate*/r39_untranslate1400） |
| 复制消息链接 | ✅ | `getMessageLink` → `clipboard` + arboard + "Link copied" toast；右键菜单仅在群/频道（linkable）显示 | `copy_link_writes_clipboard` + 实测截图（r39_copylink*） |
| 富文本实体（粗/斜/剧透/超链） | ✅ | `FormattedText.entities` → `StyledStr`（粗/斜/下划/删除/剧透/代码/引用/链接，UTF-16→byte 映射+重叠合并） | `styled_entities_merge` |
| 转发带出处徽标 | ✅ | `m.forward_info.origin`（User/HiddenUser/Chat/Channel 出处行内徽标） | — |
| 消息右键菜单（回复/编辑/复制/转发/删除/反应/置顶/选择） | ✅ | 行 context_menu → 对应 API：`ContextMenu` preview=bubble+accessory=反应条（waterui#1245 已落地；Linux 弹窗暂不含 accessory，hydrolysis#200）；Reply/Edit(仅own)/Copy/Pin·Unpin/Forward/Select/Delete(CommandRole::Destructive)；Pin 派发已实测（r33_ctx800_pin.png） | `context_menu_items_and_roles` `message_context_menu_desktop_items` `message_context_menu_edit_only_own` |
| 多选消息（批量删除/批量转发） | ✅ | `deleteMessages(revoke)` / `forwardMessages` 批处理 + 选择条（☑/☐ 行内勾选，实测 "1 selected"→"2 selected"）；选择条 Copy/Forward/Delete/✕ 实测派发（hydrolysis#239 修复 `when` payload 按钮，r37 实机验证转发流程走通） | `multi_select_bar_appears` |
| 投票显示与投票 | ✅ | `MessagePoll` → 问题/选项/得票条/已选✓/总数；`setPollAnswer` 投票 | `poll_renders_in_bubble` |
| 创建投票 | ✅ | 附件菜单 → 创建投票：问题 + 2–10 选项 + 匿名/多选/测验标记 → `sendMessage(inputMessagePoll)`，含 Regular/Quiz 两型与 correct_option_id | `poll_creator_sends_and_resets` `poll_option_remove_shifts` |
| 定时消息列表/立即发送 | ✅ | `getChatScheduledMessages` 面板 + `editMessageSchedulingState(None)`（工具栏时钟） | `scheduled_panel_lists_rows` |
| 草稿跨端同步 | ✅ | `setChatDraftMessage`（切换会话时把本地草稿推到服务器，`updateChatDraftMessage` 端已收） | `drafts_saved_per_chat` |
| 转发不带署名 | ✅ | `forwardMessages(send_copy)`（转发横幅「Without attribution」切换） | `forward_banner_has_noattr_chip` |
| 转发带评论 | ✅ | 转发横幅内评论字段 → `forwardMessages` 完成后同聊天 `sendMessage` 文本附言 | `forward_banner_shows_comment_field` |
| @提及/用户名补全 | ✅ | 输入框尾部 @token → `searchChatMembers`+`getUser`(username) 过滤弹层，选中回填 `@username ` | `mention_popup_filters_and_inserts`, `mention_token_parses` |
| 共享媒体浏览（聊天内图/视频网格） | ✅ | `searchChatMessages(filter PhotoAndVideo)` → 右侧信息面板 3 列网格 | `info_panel_shows_shared_media` |
| 信息面板共享内容标签页 | ✅ | 「Shared」区 `shared_tab` Media/Files/Links 三 tab：媒体网格 / 文档行 / 链接行，点击行内跳转对应消息 | `info_panel_shows_shared_media` `probe_overlay_chunk` + 实测截图（r37_info*/_files/_links） |
| 未读消息分隔线 | ✅ | `chat.last_read_inbox_message_id`+`unread_count` → 首条未读上方「Unread messages」分隔条 | `unread_divider_renders` |
| 打开会话锚定未读分隔线 | ✅ | `scroll_to_open`：unread>0 且有分隔线 → 定位分隔线行；未读挂起期间收到新消息不动视口（`follows_tail`），自发消息仍跟随尾部 | `open_unread_anchors_divider` |
| 发送者头像/名字、转发徽标点开资料 | ✅ | 气泡列头像、组内发送者名、「Forwarded from」徽标 → `open_peer` → Profile 卡（demo 合成卡） | `peer_taps_open_profiles` |
| `:emoji` 短码补全 | ✅ | 输入框尾部 `:token`(≥2 字符) → `EMOJI_SHORTCODES` 前缀建议条（≤6 项），点选替换 token | `emoji_autocomplete_inserts` |
| @提及跳转按钮 | ✅ | `unread_mentions>0` → 右下浮动 @ 圆钮 → `mention_jump`：跳转 `mentions_me` 消息并清未读提及 | `chat_row_mention_badge` `mention_jump_targets_and_clears` |
| 未读回应徽标 + 跳转 | ✅ | `updateChatUnreadReactionCount`/`updateMessageUnreadReactions` → 行内 ❤ 徽标 + 会话内浮动 ❤ 圆钮（@ 之下）→ `reaction_jump` 跳转首条未读回应消息并清计数（行徽标重绘受 hydrolysis#227 挂载行不重测所限） | `reaction_jump_jumps_and_clears` + 实测截图（r39_base*/r39_reactjump*） |
| 悬停快捷回复钮 | ✅ | 指针悬停气泡 → 旁侧 ↩ 圆钮 → 一键回复（Telegram Desktop hover affordance） | 实测截图（r35_hover1400/800/600） |
| 双击快捷回应 | ✅ | 双击气泡 → `quick_react` 切换 ❤️（Desktop 默认快反应） | `quick_react_applies_heart` |
| 侧边栏搜索命中高亮 | ✅ | `title_styled`/`preview_styled` = 命中子串 AccentContainer span 标记；行级重绘受 DOGFOOD r35-3 保留路径缺陷所限 | `sidebar_search_highlight_splits` |
| 全局消息搜索（"Messages" 区） | ✅ | `searchMessages(ChatList::Main)` → 侧栏结果区 "Messages" 分区（标题+发送者:摘要+时间）；点击 `open_hit` = `select_chat`+`jump_to_message` 高亮跳消息；demo 走语料扫描 | `global_message_search_demo` |
| 未读回到底部浮动钮 | ✅ | 打开的会话 unread>0 → 右下 "↓ N" 圆钮 → `catch_up` = `scroll_bottom`+`mark_read`+文件夹徽标重算；scroll 读回缺失（waterui#1259）故暂按 unread>0 常驻显示 | `catch_up_chip_marks_read` |
| 静音会话样式 | ✅ | `row.muted` → 标题 MutedForeground + `bell_off` 图标，未读徽标灰底（SurfaceVariant/MutedForeground 而非 Accent） | `mute_toggles_row_style` |
| 文件夹未读徽标 | ✅ | `updateUnreadChatCount` → `folder_unreads` 按 Main/Archive/Folder 键记入，chip 标题后加未读会话数；demo 由 `demo_recount_folders` 从名册统计 | `folder_unread_badges` |
| 操作反馈 toast | ✅ | `SnackbarManager` + `.snackbar(...)` 浮层：复制/置顶/转发完成/删除等提示（"2 messages forwarded" 实测出现）；按 Desktop 底部居中条 | `toast_notice_fires_on_copy` + 实测截图（r37_snack*/r37d_fwded.png） |
| 频道贴 footer（浏览数/签名） | ✅ | `kind_icon="channel"` 贴子行尾：👁 `view_count` + `author_signature` | `channel_post_footer_shows_views` + 实测截图（r37_channel*） |
| 跳转到日期弹层 | ✅ | 工具栏日历钮 → 日期弹层 → `date_jump_target` 定位并高亮目标消息 | `date_jump_popup_opens` + 实测截图（r37_jump*） |
| 拖拽文件进窗口发送 | ❌ | 框架双层缺口：waterui `DragData` 仅 Text|Url 无 File；hydrolysis 未桥接 winit DroppedFile/HoveredFile — DOGFOOD r34-1 | — |
| 日期分隔条 | ✅ | 消息 `date` 跨天时插入居中分隔（Today / Yesterday / 月 日 / 月 日， 年） | `set_messages_marks_day_headers` |
| 消息分组（同发送者连发折叠 + 头像列 + 组末气泡尾巴） | ✅ | 连续同向同发送者合并为一组：发送者名仅显示于组内首条并按对端 accent 色着色；群组/频道内组末条底部显示发送者头像（`sender_photo` 或首字母色圆）；组末气泡底角尾巴为气泡背景层内 Path 楔形 + `.offset` 外推（同 fill 一体、接角方角、不占布局空间）；组间间距大于组内 | `set_messages_groups_runs` |

## 4. 媒体与附件

| 功能 | 状态 | TDLib API | 测试 |
|---|---|---|---|
| 发送图片 | ✅ | `sendMessage(inputMessagePhoto)`（FilePicker，气泡内 Photo 渲染） | `attachment_content_dispatch` |
| 发送视频 | ✅ | `sendMessage(inputMessageVideo)`（按扩展名分发） | `attachment_content_dispatch` |
| 发送文件 | ✅ | `sendMessage(inputMessageDocument)`（默认兜底） | `attachment_content_dispatch` |
| 语音消息（录制/发送/播放） | ✅ | 播放：`video_player`；录制：waterkit-audio cpal → opus-pure Ogg/Opus + waveform → `inputMessageVoiceNote` | `opus_ogg_container`、`waveform_encodes_100_bars`、`voice_record_button_handles_missing_device` |
| 视频消息 | ✅ | 播放：`video_player`；录制：GpuSurface/GpuView 直渲预览（device.clone → Arc，帧留 GPU）→ compute pass NV12 → waterkit-codec H.264 → VideoWriter mp4 → `inputMessageVideoNote`（VA-API 编码需 /dev/dri） | `nv12_layout`、`crop_square`、`video_note_handles_missing_camera` |
| 贴纸 | ✅ | `getRecentStickers`/`searchStickers`/`getInstalledStickerSets`/`getStickerSet`（emoji 搜索+贴纸包浏览）→ `inputMessageSticker` | `sticker_picker_toggles` |
| GIF | ✅ | `getSavedAnimations` + @gif inline bot `getInlineQueryResults` trending 搜索 → `inputMessageAnimation` | `sticker_picker_toggles` |
| 图片/视频气泡内预览 | ✅ | `downloadFile` → `file_signal`；图片 `Photo`（圆角裁剪、max_width 320）、视频/动画 `video_player`、音频紧凑播放器；demo 种子含程序化 640×360 PNG 实图渲染验证 | `media_slot`、`media_play_fallback_row`、`demo_seeds_photo_file` |
| 下载进度指示 | ✅ | `updateFile` → `file_progress`（`label — N%`） | — |
| 相册多选发送 | ✅ | `sendMessageAlbum`（≥2 媒体文件合并为一条相册；混合类型逐个发，标题落在首条） | `attachment_planning` |
| 接收端相册合并气泡 | ✅ | `media_album_id` 同组消息合并为一条气泡，双列网格渲染所有成员媒体（`album_files`→`file_signal`，空路径按位占位符——`Url::from_file_path_str` 空串 panic 已按 `has`-gate 规避） | `album_rows_merge_into_one_bubble` + 实测截图（r37_album800/600） |
| 媒体查看器（点图大图/播放） | ✅ | 覆盖层 zstack：Photo/`video_player` + 发送者/说明/关闭（占满会话面板；非全屏） | `media_viewer_overlay` |
| 复制图片到剪贴板 | ✅ | 右键菜单「Copy image」（仅图片行）→ `file_signal` 快照路径 → `image::open`→`to_rgba8` → arboard `set_image` + "Image copied" toast | `copy_image_decodes_demo_photo` + 实测截图（r39_copyimg*） |
| Emoji 选择面板 | ✅ | 本地 Emoji 网格（~300 项，VS16 整段）→ 插入输入框；与贴纸/GIF 同一面板三 tab | `emoji_tab_shows_grid` |
| 发送前预览+说明编辑 | ✅ | 附件条：图片缩略图+文件名+caption 输入框（发送时并入消息 caption） | `attach_preview_shows_caption_field` |

## 5. 频道与群组

| 功能 | 状态 | TDLib API | 测试 |
|---|---|---|---|
| 新建私聊 | ✅ | `createPrivateChat` | — |
| 发起加密聊天 | ✅ | `createNewSecretChat`（联系人右键菜单） | — |
| 新建群组 | ✅ | `createNewSupergroupChat` | — |
| 公开频道浏览/搜索加入 | ✅ | `searchChatsOnServer` + `joinChat`（行菜单） | — |
| 加入/退出 | ✅ | `joinChat`/`leaveChat` | — |
| 成员列表 | ✅ | `searchChatMembers`（名字+角色面板）；宽布局右侧信息面板（头像/成员数/共享媒体） | `members_panel_lists_members` `info_panel_shows_shared_media` |
| 基础管理（改标题/描述/头像、删成员） | ✅ | `setChatTitle`/`setChatDescription`/`setChatPhoto`/`setChatMemberStatus(Banned)`/`setMessageSenderBlockList` | — |
| 频道简介/邀请链接 | ✅ | `createChatInviteLink` → 成员面板显示链接（New link 按钮） | `members_invite_row` |

## 6. 联系人与个人资料

| 功能 | 状态 | TDLib API | 测试 |
|---|---|---|---|
| 联系人列表 | ✅ | `getContacts` + `getUser`（新建聊天页，点击开聊） | `contacts_list_renders` |
| 添加/删除联系人 | ✅ | `importContacts`（电话+姓名表单）/`removeContacts`（行右键菜单） | `contacts_add_form_opens` |
| 查看对方资料 | ✅ | `getUser` + `getUserFullInfo` → Profile 路由（名/用户名/电话/简介/在线 + Message 按钮） | `profile_view_renders` |
| 编辑自己资料（名/简介/用户名/头像） | ✅ | `setName`/`setBio`/`setUsername` + `setProfilePhoto`（Settings 选图上传） | `settings_shows_sections` |

## 7. 设置

| 功能 | 状态 | TDLib API | 测试 |
|---|---|---|---|
| 深色模式 | ✅ | env `ColorScheme` | `dark_mode_toggle` |
| 账号信息展示 | ✅ | `getMe` | `settings_shows_account` |
| 通知设置（全局/按聊天） | ✅ | `setChatNotificationSettings`（单聊）+ `setScopeNotificationSettings`（私聊/群/频道三项全局开关） | — |
| 隐私设置 | ✅ | `getUserPrivacySettingRules` + `setUserPrivacySettingRules`：预设 + 逐用户 Always/Never allow 例外（联系人选择器，AllowUsers/RestrictUsers 合并前置） | `privacy_audience_mapping`、`privacy_exception_merge` |
| 两步验证管理 | ✅ | `getPasswordState` + `setPassword`（设/改/关 + hint + recovery email；SRP 由 TDLib 内部计算） | `twofa_requires_current_password` |
| 活跃会话管理 | ✅ | `getActiveSessions` + `terminateSession`/`terminateAllOtherSessions` | `settings_shows_sections` |
| 语言包 | ✅ | `getLocalizationTargetInfo` 列表 + `setOption(language_pack_id)` 切换 + `getLanguagePackStrings` 全量拉取喂 `Store::tr`（复数按 zero/one/other 槽选；更新经 `UpdateLanguagePackStrings` 合入；官方包键未覆盖处回落英文原文） | `lang_pack_section` |
| 存储/缓存清理 | ✅ | `getStorageStatistics` 摘要 + `optimizeStorage` 清理按钮 | `settings_storage_section` |
| 屏蔽用户管理 | ✅ | `getBlockedMessageSenders` + `setMessageSenderBlockList(None)` 解除（Privacy 区块） | `settings_blocked_section` |
| 会话右键菜单（已读/未读/置顶/归档/静音/加入/离开/清空历史） | ✅ | 行 context_menu；`deleteChatHistory` 清空历史；行悬停 ⋮ 快捷菜单复用同一命令集（`Menu::new` + `.icon_only()`）—— 悬停显现 ⋮（悬停时时间戳隐藏，Desktop 行为）且点击实测打开菜单（r36_hovmenu1400/800/600.png） | `probe_hover_enter_fires` + 实测截图 |
| 收藏夹（Saved Messages） | ✅ | TDLib 中即本人私聊，走普通会话路径 | — |

## 8. 明确不计划（本期范围外）

语音/视频通话（`call_*`）、Stories、支付/商品、Bot 内联查询（GIF bot 除外）、Live Location、Passport、Proxy 设置、导出聊天历史、话题/Forum。

## 覆盖情况汇总

- 已实现 ✅：113 项 ｜ 部分 🟡：0 项 ｜ 未实现 ❌：2 项（拖放文件发送 DOGFOOD r34-1，全局快捷键 DOGFOOD r36-1）
- r39 新增：自动删除定时器（行菜单嵌套 `Menu`）、未读回应 ❤ 徽标+浮动跳转钮、消息翻译（内联互换+还原）、复制消息链接、复制图片 —— 五项均 1400/800/600 实机验证
- 现有测试：19 个 `#[waterui::test]` + 探测测试 + 1 个 `#[ignore]` 真实 DC e2e（`tests/tdlib_e2e.rs`）
- r8 重审补行：会话右键菜单、多选批处理、投票、定时消息面板、媒体查看器、Emoji 面板、草稿同步、转发无署名、屏蔽用户、加密聊天、Saved Messages、清空历史；r10 补录：右侧信息面板（共享媒体网格）、未读分隔线、发送前缩略图+caption。r12 补录：创建投票；r13 补录：语言包（官方键未覆盖的串回落英文）。剩余 ❌：无。r11 补录：@提及补全、转发附言、右侧信息面板窄窗阈值（<1120 覆盖式 / ≥1120 内嵌）。r12 补录：创建投票（创建面板 + Regular/Quiz 两型），并回退三处 r11 缓解恢复框架复现（nami#23 / waterui#1214 / hydrolysis#129 即 DOGFOOD 对应条目）。r34 补录：列表键盘导航、打开未读锚定、头像/名字/转发徽标点资料、`:emoji` 补全、链接卡片可点；拖放文件发送记为 ❌（DOGFOOD r34-1 框架双层缺口）。r35 补录：消息内搜索 n/N+^/v+命中高亮、@提及跳转钮、悬停↩快捷回复、双击❤️快反应、侧边栏搜索命中高亮。r36 补录：置顶消息多选弹层实测（r36_pinned1400/800/600）、会话行悬停 ⋮ 菜单实测（r36_hovmenu*，悬停时时间戳隐藏）、composer Markdown→实体+demo 回显实测（r36_echo*）、多选条实测部分（r36-5 死按钮记 🟡）、全局快捷键记 ❌（r36-1）。搜索命中高亮配色修正为 M3 container+on-*（r36_marks*）。r38 补录：全局消息搜索 "Messages" 区、删除确认卡（私聊 revoke 勾选）、未读回到底部浮动钮、静音样式（bell_off+暗化标题+灰徽标）、文件夹未读徽标（`updateUnreadChatCount`）。
- r37 补录：toast 提示（Snackbar）、频道贴 footer（👁 浏览数+签名）、跳转到日期弹层、相册合并气泡、信息面板共享 Media/Files/Links 标签页；多选批处理升 ✅（hydrolysis#239 修复实测）。
