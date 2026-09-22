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
| 多账号切换 | ❌ | 多 `client_id` | — |
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
| 归档（Archive 文件夹） | ❌ | `ChatList::Archive` + `toggleChatIsPinned` 等位 | — |
| 聊天文件夹（Chat Folders） | ❌ | `getChatFolderChatsToLeave`/`chatFolderInfo`/`createChatFolder` | — |
| 已读回执标记 | 🟡 | `openChat`/`closeChat`/`viewMessages`（单向，无 UI 标记 out） | — |
| 已标记为未读 | ❌ | `toggleChatIsMarkedAsUnread` | — |

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
| 消息内搜索 | ❌ | `searchChatMessages` | — |
| 跳转到某条消息（日期/回复定位） | ❌ | `getChatHistory(from_message_id)` 定位模式 | — |
| 置顶消息条 | ❌ | `chat.pinned_message`/`getChatPinnedMessage`/`pinChatMessage`/`unpinChatMessage` | — |
| Reactions 展示 | 🟡 | `m.interaction_info.reactions`（仅文本展示） | — |
| Reactions 发送 | ❌ | `addMessageReaction`/`removeMessageReaction` | — |
| 发送中/失败状态 | 🟡 | `updateMessageSendSucceeded`/`Failed`（行内） | — |
| 定时消息 / 静默发送 | ❌ | `sendMessage(scheduling_state)` | — |
| 链接预览 | ❌ | `getWebPagePreview`/`link_preview` | — |
| 富文本实体（粗/斜/剧透/超链） | ❌ | `formattedText.entities`（当前只拼 plain text） | — |
| 转发带出处徽标 | 🟡 | `m.forward_info`（仅行内标签） | — |

## 4. 媒体与附件

| 功能 | 状态 | TDLib API | 测试 |
|---|---|---|---|
| 发送图片 | 🟡 | `sendMessage(inputMessagePhoto)`（FilePicker，无缩略图预览） | — |
| 发送视频 | ❌ | `inputMessageVideo` | — |
| 发送文件 | ❌ | `inputMessageDocument` | — |
| 语音消息（录制/发送/播放） | ❌ | `inputMessageVoiceNote` + `sendChatActionRecordingVoiceNote` | — |
| 视频消息 | ❌ | `inputMessageVideoNote` | — |
| 贴纸 | ❌ | `inputMessageSticker` + `getStickers` | — |
| GIF | ❌ | `inputMessageAnimation` + `getSavedAnimations` | — |
| 图片/视频气泡内预览 | 🟡 | `downloadFile` → `file_signal`（仅占位符+路径，无图像渲染） | — |
| 下载进度指示 | 🟡 | `updateFile`（进度字段已收，UI 未展示） | — |
| 相册多选发送 | ❌ | `sendMessageAlbum` | — |

## 5. 频道与群组

| 功能 | 状态 | TDLib API | 测试 |
|---|---|---|---|
| 新建私聊 | ✅ | `createPrivateChat` | — |
| 新建群组 | ✅ | `createNewSupergroupChat` | — |
| 公开频道浏览/搜索加入 | 🟡 | `searchChatsOnServer` + `joinChat` | — |
| 加入/退出 | ✅ | `joinChat`/`leaveChat` | — |
| 成员列表 | ❌ | `getSupergroupMembers`/`searchChatMembers` | — |
| 基础管理（改标题/描述/头像、删成员） | ❌ | `setChatTitle`/`setChatDescription`/`setChatPhoto`/`banChatMember`/`setChatMemberStatus` | — |
| 频道简介/邀请链接 | ❌ | `getChatInviteLink`/`createChatInviteLink` | — |

## 6. 联系人与个人资料

| 功能 | 状态 | TDLib API | 测试 |
|---|---|---|---|
| 联系人列表 | ❌ | `getContacts`/`importContacts`/`searchChatsOnServer` | — |
| 添加/删除联系人 | ❌ | `addContact`/`removeContacts` | — |
| 查看对方资料 | 🟡 | `getUser`（仅名字显示） | — |
| 编辑自己资料（名/简介/用户名/头像） | ❌ | `setName`/`setBio`/`setUsername`/`setProfilePhoto` | — |

## 7. 设置

| 功能 | 状态 | TDLib API | 测试 |
|---|---|---|---|
| 深色模式 | ✅ | env `ColorScheme` | `dark_mode_toggle` |
| 账号信息展示 | ✅ | `getMe` | `settings_shows_account` |
| 通知设置（全局/按聊天） | 🟡 | `setChatNotificationSettings`（仅单聊切换） | — |
| 隐私设置 | ❌ | `getUserPrivacySettingRules`/`setUserPrivacySettingRules` | — |
| 两步验证管理 | ❌ | `getPasswordState`/`setPassword` | — |
| 活跃会话管理 | ❌ | `getActiveSessions`/`terminateSession`/`terminateAllOtherSessions` | — |
| 语言 | ❌ | `setOption(language_pack_id)` | — |
| 存储/缓存清理 | ❌ | `getStorageStatistics`/`optimizeStorage` | — |

## 8. 明确不计划（本期范围外）

语音/视频通话（`call_*`）、Stories、支付/商品、Bot 内联查询、Poll 投票 UI、Live Location、Passport、Proxy 设置、导出聊天历史。

## 覆盖情况汇总

- 已实现 ✅：26 项 ｜ 部分 🟡：12 项 ｜ 未实现 ❌：29 项
- 现有测试：11 个 `#[waterui::test]` + 1 个 `#[ignore]` 真实 DC e2e（`tests/tdlib_e2e.rs`）
