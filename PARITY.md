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
| 未读数 / 提及徽标 | ✅ | `Chat.unread_count`/`unread_mention_count` | `chat_list_rows` |
| 置顶聊天 | ✅ | `toggleChatIsPinned` | — |
| 静音 / 通知设置 | ✅ | `setChatNotificationSettings` | — |
| 头像 + 在线状态点 | ✅ | `chat.photo`/`user.status` + `downloadFile` | `chat_list_rows` |
| typing/… 指示 | ✅ | `sendChatAction` + `updateChatAction` | — |
| 草稿显示 | ✅ | `updateChatDraftMessage` | — |
| 归档（Archive 文件夹） | ✅ | `addChatToList(Main/Archive)` + `ChatPosition` | `archive_toggle_rebuilds_list` |
| 聊天文件夹（Chat Folders） | ✅ | `updateChatFolders` + `ChatList::Folder`/`loadChats` 侧栏 tab；`createChatFolder`/`editChatFolder`/`deleteChatFolder`（名称+联系人群组频道开关；不含逐聊包含/排除编辑） | `folder_tabs_render`、`folder_editor_opens` |
| 已读回执标记 | ✅ | `viewMessages` + `UpdateChatReadOutbox` → ✓/✓✓ | `read_receipt_double_check` |
| 已标记为未读 | ✅ | `toggleChatIsMarkedAsUnread` + `UpdateChatIsMarkedAsUnread` | `marked_unread_shows_dot` |

## 3. 会话与消息

| 功能 | 状态 | TDLib API | 测试 |
|---|---|---|---|
| 打开会话 + 历史分页 | ✅ | `getChatHistory` 向前翻页 | `messages_render` |
| 发送文本 | ✅ | `sendMessage(inputMessageText)` | `composer_sends_and_clears` |
| 编辑消息 | ✅ | `editMessageText` | — |
| 删除消息 | ✅ | `deleteMessages` | — |
| 转发 | ✅ | `forwardMessages` | — |
| 回复 / 引用 | ✅ | `inputMessageReplyTo` | `reply_banner_shows` |
| 复制文本 | ✅ | `getMessage` + 剪贴板 | — |
| 已读标记（拉取后回执） | ✅ | `viewMessages` | — |
| 消息内搜索 | ✅ | `searchChatMessages`（输入防抖 400ms） | `chat_search_panel_opens` |
| 跳转到某条消息（日期/回复定位） | ✅ | `getChatHistory` 窗口加载 + `List` `ScrollController<usize>` 按索引精确滚动 + 气泡高亮 | — |
| 置顶消息条 | ✅ | `getChatPinnedMessage` + `pinChatMessage`/`unpinChatMessage`，点击跳转 | `pinned_banner_shows` |
| Reactions 展示 | ✅ | `UpdateMessageInteractionInfo`（数量+自己的选择） | `reactions_render` |
| Reactions 发送 | ✅ | `addMessageReaction`/`removeMessageReaction`（右键菜单 👍❤️😂😮😢 + 取消） | — |
| 发送中/失败状态 | ✅ | `updateMessageSendSucceeded`/`Failed` 行内图标；点 ✗ → `resendMessages` | `resend_failed_marks_pending` |
| 定时消息 / 静默发送 | ✅ | `sendMessage(MessageSendOptions{disable_notification,scheduling_state=SendAtDate})` — 发送按钮右键 | — (send_opt 路径） |
| 链接预览 | ✅ | `MessageText.link_preview` → 站内一行卡片（site—title·desc) | `link_preview_line_shows` |
| 富文本实体（粗/斜/剧透/超链） | ✅ | `FormattedText.entities` → `StyledStr`（粗/斜/下划/删除/剧透/代码/引用/链接，UTF-16→byte 映射+重叠合并） | `styled_entities_merge` |
| 转发带出处徽标 | ✅ | `m.forward_info.origin`（User/HiddenUser/Chat/Channel 出处行内徽标） | — |
| 消息右键菜单（回复/编辑/复制/转发/删除/反应/置顶/选择） | ✅ | 行 context_menu → 对应 API | — |
| 多选消息（批量删除/批量转发） | ✅ | `deleteMessages(revoke)` / `forwardMessages` 批处理 + 选择条（☑/☐ 行内勾选） | `multi_select_bar_appears` |
| 投票显示与投票 | ✅ | `MessagePoll` → 问题/选项/得票条/已选✓/总数；`setPollAnswer` 投票 | `poll_renders_in_bubble` |
| 创建投票 | ✅ | 附件菜单 → 创建投票：问题 + 2–10 选项 + 匿名/多选/测验标记 → `sendMessage(inputMessagePoll)`，含 Regular/Quiz 两型与 correct_option_id | `poll_creator_sends_and_resets` `poll_option_remove_shifts` |
| 定时消息列表/立即发送 | ✅ | `getChatScheduledMessages` 面板 + `editMessageSchedulingState(None)`（工具栏时钟） | `scheduled_panel_lists_rows` |
| 草稿跨端同步 | ✅ | `setChatDraftMessage`（切换会话时把本地草稿推到服务器，`updateChatDraftMessage` 端已收） | `drafts_saved_per_chat` |
| 转发不带署名 | ✅ | `forwardMessages(send_copy)`（转发横幅「Without attribution」切换） | `forward_banner_has_noattr_chip` |
| 转发带评论 | ✅ | 转发横幅内评论字段 → `forwardMessages` 完成后同聊天 `sendMessage` 文本附言 | `forward_banner_shows_comment_field` |
| @提及/用户名补全 | ✅ | 输入框尾部 @token → `searchChatMembers`+`getUser`(username) 过滤弹层，选中回填 `@username ` | `mention_popup_filters_and_inserts`, `mention_token_parses` |
| 共享媒体浏览（聊天内图/视频网格） | ✅ | `searchChatMessages(filter PhotoAndVideo)` → 右侧信息面板 3 列网格 | `info_panel_shows_shared_media` |
| 未读消息分隔线 | ✅ | `chat.last_read_inbox_message_id`+`unread_count` → 首条未读上方「Unread messages」分隔条 | `unread_divider_renders` |

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
| 图片/视频气泡内预览 | ✅ | `downloadFile` → `file_signal`；图片 `Photo`、视频/动画 `video_player`、音频紧凑播放器 | `media_slot`、`media_play_fallback_row` |
| 下载进度指示 | ✅ | `updateFile` → `file_progress`（`label — N%`） | — |
| 相册多选发送 | ✅ | `sendMessageAlbum`（≥2 媒体文件合并为一条相册；混合类型逐个发，标题落在首条） | `attachment_planning` |
| 媒体查看器（点图大图/播放） | ✅ | 覆盖层 zstack：Photo/`video_player` + 发送者/说明/关闭（占满会话面板；非全屏） | `media_viewer_overlay` |
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
| 会话右键菜单（已读/未读/置顶/归档/静音/加入/离开/清空历史） | ✅ | 行 context_menu；`deleteChatHistory` 清空历史 | — |
| 收藏夹（Saved Messages） | ✅ | TDLib 中即本人私聊，走普通会话路径 | — |

## 8. 明确不计划（本期范围外）

语音/视频通话（`call_*`）、Stories、支付/商品、Bot 内联查询（GIF bot 除外）、Live Location、Passport、Proxy 设置、导出聊天历史、话题/Forum。

## 覆盖情况汇总

- 已实现 ✅：84 项 ｜ 部分 🟡：0 项 ｜ 未实现 ❌：0 项
- 现有测试：18 个 `#[waterui::test]` + 探测测试 + 1 个 `#[ignore]` 真实 DC e2e（`tests/tdlib_e2e.rs`）
- r8 重审补行：会话右键菜单、多选批处理、投票、定时消息面板、媒体查看器、Emoji 面板、草稿同步、转发无署名、屏蔽用户、加密聊天、Saved Messages、清空历史；r10 补录：右侧信息面板（共享媒体网格）、未读分隔线、发送前缩略图+caption。r12 补录：创建投票；r13 补录：语言包（官方键未覆盖的串回落英文）。剩余 ❌：无。r11 补录：@提及补全、转发附言、右侧信息面板窄窗阈值（<1120 覆盖式 / ≥1120 内嵌）。r12 补录：创建投票（创建面板 + Regular/Quiz 两型），并回退三处 r11 缓解恢复框架复现（nami#23 / waterui#1214 / hydrolysis#129 即 DOGFOOD 对应条目）。
