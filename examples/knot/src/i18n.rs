//! Lightweight in-app internationalization (Japanese / English + System).
//!
//! A direct port of Knot v0.7.0's hand-rolled i18n (`src/i18n/`): a flat
//! key → string table for two languages, no external crate. The original
//! kept this deliberately small ("not react-i18next — intentionally
//! lightweight for ~85 keys, 2 languages"); the same reasoning holds here.
//!
//! The chosen language is a non-secret setting, so it lives next to theme /
//! font-size in [`crate::settings`] (the persisted [`Language`] field +
//! its live [`shroud::reactive::Signal`]). This module owns only the
//! translation *tables* and the lookup ([`tr`]); the signal it reads is
//! [`crate::settings::signals`]`().language`.
//!
//! Switching language is **live**, exactly like a theme swap: every visible
//! string is rendered through a reactive constructor
//! ([`shroud::widgets::TextWidget::reactive`] /
//! [`shroud::widgets::Button::reactive_label`]) whose closure calls [`tr`]
//! on each paint, so flipping the setting re-renders the UI without a tree
//! rebuild. The one exception is `Input` placeholders, which the framework
//! takes as a plain `String` — those resolve at screen-build time, so a
//! placeholder behind the open settings modal keeps its old-language text
//! until that screen is next built (a lock cycle, or app relaunch). Minor,
//! and language is typically a set-once preference.
//!
//! ## Usage
//!
//! * Reactive text:  `TextWidget::reactive(move || tr(Key::Foo).to_string())`
//! * Reactive label: `Button::reactive_label(move || tr(Key::Foo).to_string())`
//! * Placeholder:    `.placeholder(tr(Key::Foo))`  (one-shot, see above)
//! * Parameterized:  `tr(Key::Foo).replace("{n}", &n.to_string())`
//! * Prefix + detail (errors): `format!("{}{e}", tr(Key::FooPrefix))`
//!
//! When adding a user-facing string, add a [`Key`] variant and fill in
//! **both** arms of [`Key::ja`] and [`Key::en`] (the `match`es are
//! exhaustive, so a missing arm is a compile error — the Rust equivalent of
//! the upstream "add entries to both `ja` and `en`" rule).

use serde::{Deserialize, Serialize};

/// Stored language preference. `System` follows the OS locale (a `ja*` BCP-47
/// tag resolves to Japanese, anything else to English — matching the
/// upstream `resolvedLanguage` logic). Persisted in `settings.json`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    Ja,
    En,
    #[default]
    System,
}

/// The concrete language a lookup actually resolves to (never `System`).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Lang {
    Ja,
    En,
}

/// Resolve the stored preference into a concrete [`Lang`], consulting the OS
/// locale for `System`. Read on every paint by [`tr`], so it must be cheap;
/// `system_locale()` is a thin OS query and only runs in the `System` arm.
fn resolved() -> Lang {
    match crate::settings::signals().language.get() {
        Language::Ja => Lang::Ja,
        Language::En => Lang::En,
        Language::System => match shroud::platform::system_locale() {
            Some(tag) if tag.to_ascii_lowercase().starts_with("ja") => Lang::Ja,
            _ => Lang::En,
        },
    }
}

/// Translate a [`Key`] into the resolved language's string.
///
/// Returns a `&'static str` so it drops straight into a placeholder
/// (`impl Into<String>`) and costs nothing in the common (non-parameterized)
/// case; reactive callers `.to_string()` it for the closure's `String`
/// return.
pub fn tr(key: Key) -> &'static str {
    match resolved() {
        Lang::Ja => key.ja(),
        Lang::En => key.en(),
    }
}

/// Every user-facing string in the app. Variants grouped by screen; shared
/// strings (used on more than one screen) live in the `// Shared` block.
///
/// Parameterized strings carry a `{n}` / `{title}` / `{label}` token the
/// caller substitutes with `str::replace`; error strings that append an OS
/// detail end in a separator and are used as a `format!` prefix.
#[derive(Clone, Copy)]
pub enum Key {
    // --- Shared ---
    /// `"(untitled)"` — empty-title fallback (editor header + sidebar row).
    Untitled,
    /// `"New master password"` — setup + recovery password field.
    NewPasswordPlaceholder,
    /// `"password must be at least {n} characters"` — setup + recovery.
    ValidationMinLength,
    /// `"passwords don't match — try again"` — setup + recovery.
    PasswordsMismatch,
    /// `"failed to write salt: "` — setup + recovery (prefix).
    ErrWriteSaltPrefix,
    /// `"failed to write key file: "` — setup + recovery (prefix).
    ErrWriteKeyFilePrefix,
    /// `"failed to read notes: "` — lock + recovery (prefix).
    ErrReadNotesPrefix,
    /// `"failed to derive recovery key"` — setup + recovery.
    ErrDeriveRecovery,

    // --- Lock screen ---
    Tagline,
    LockMasterPasswordLabel,
    LockPasswordPlaceholder,
    Locked,
    /// `"Locked — "` (prefix; an error detail is appended).
    LockedErrorPrefix,
    /// `"Too many attempts — try again in {n}s"`.
    TooManyAttempts,
    ForgotPassword,
    ConfigUnavailable,
    LockWrongPassword,
    LockKeyMismatch,
    ErrReadKeyFilePrefix,
    ErrVaultPrefix,

    // --- Setup screen ---
    SetupDescription,
    /// `"At least {n} characters. …"`.
    SetupHint,
    ConfirmPasswordPlaceholder,
    SetupChoosePrompt,
    SetupConfirmPrompt,
    ConfirmFirstSetup,
    ErrWriteRecoveryPrefix,
    ErrCreateVaultPrefix,
    ErrInitVaultPrefix,

    // --- Recovery-key reveal (last setup step) ---
    RecoveryRevealTitle,
    RecoveryRevealDescription,
    RecoveryRevealWarning,
    RecoveryRevealDone,

    // --- Recovery screen ---
    RecoveryTitle,
    RecoveryDescription,
    RecoveryKeyPlaceholder,
    RecoveryConfirmPlaceholder,
    RecoveryPrompt,
    RecoveryBack,
    ConfirmFirstRecovery,
    /// `"recovery key must be {n} words"`.
    RecoveryWordCount,
    RecoveryKeyInvalid,
    RecoveryNotSetUp,
    VaultDataCorrupted,
    ErrOpenVaultPrefix,

    // --- Editor ---
    EditorImageBtn,
    EditorExportBtn,
    EditorLockBtn,
    EditorPreviewShow,
    EditorPreviewHide,
    EditorTitlePlaceholder,
    EditorBodyPlaceholder,
    /// `"Editing: {title}"`.
    EditorEditing,
    EditorNoNoteSelected,
    EditorNoNoteSelectedHint,
    /// `"Backlinks"` — header of the editor's backlinks panel (a count in
    /// parentheses is appended by the caller, e.g. "Backlinks (2)").
    BacklinksTitle,
    DialogInsertImage,
    DialogExportNote,
    ErrReadImagePrefix,
    ErrImageUnsupported,
    ErrImageStore,
    ErrExportPrefix,

    // --- Editor formatting toolbar ---
    // Button labels for the bar above the body. The Markdown each button
    // inserts (`**`, `# `, `[](https://)`, …) is fixed syntax, not localized.
    ToolbarHeading,
    ToolbarBold,
    ToolbarItalic,
    ToolbarCode,
    ToolbarQuote,
    ToolbarList,
    ToolbarLink,

    // --- Sidebar ---
    SidebarImport,
    SidebarNewNote,
    SidebarSearchPlaceholder,
    SidebarSort,
    SidebarSettings,
    SidebarClear,
    SidebarNoNotesYet,
    SidebarNoMatchSearchTags,
    SidebarNoMatchSearch,
    SidebarNoMatchTags,
    DialogImportNote,
    /// `"That file is too large to import (max {n} MiB)."`.
    ErrImportTooLarge,
    ErrReadFilePrefix,
    ErrDeleteNotePrefix,

    // --- Sort labels ---
    SortCreated,
    SortTitleAsc,
    SortTitleDesc,

    // --- Settings modal ---
    SettingsTitle,
    SettingsTheme,
    SettingsThemeLight,
    SettingsThemeDark,
    SettingsThemeSystem,
    SettingsFontSize,
    SettingsFontSmall,
    SettingsFontMedium,
    SettingsFontLarge,
    SettingsAutoLock,
    AutoLockOff,
    AutoLockOneMinute,
    AutoLockFiveMinutes,
    AutoLockFifteenMinutes,
    SettingsLanguage,
    LanguageJa,
    LanguageEn,
    LanguageSystem,
    SettingsDone,

    // --- Preview ---
    PreviewEmpty,
    PreviewImageUnavailable,
    /// `"[external image: {label}]"`.
    PreviewExternalImage,

    // --- Tag editor ---
    TagsAddPlaceholder,

    // --- Change password / Security ---
    SettingsSecurity,
    SettingsChangePassword,
    ChangePasswordTitle,
    ChangePasswordDescription,
    ChangePasswordCurrentPlaceholder,
    ChangePasswordPrompt,
    ChangePasswordCurrentWrong,
    ChangePasswordEnterCurrentFirst,
    ChangePasswordSuccess,
    ChangePasswordCancel,

    // --- Backup & restore ---
    SettingsBackup,
    SettingsBackupBtn,
    BackupTitle,
    BackupDescription,
    BackupFolderLabel,
    /// Shown in place of a path when no custom folder is set.
    BackupFolderDefault,
    BackupChangeFolder,
    BackupRetentionLabel,
    BackupNowBtn,
    BackupRestoreBtn,
    /// `"Backup saved to {path}"`.
    BackupSuccess,
    ErrBackupPrefix,
    ErrRestorePrefix,
    RestoreConfirmTitle,
    RestoreConfirmBody,
    RestoreConfirmBtn,
    RestoreCancel,
    /// File-picker dialog title for choosing a backup to restore.
    DialogRestoreBackup,
    BackupAutoLabel,
    AutoBackupOff,
    AutoBackupDaily,
    AutoBackupWeekly,

    // --- main / app ---
    ErrSaveChangesPrefix,
}

impl Key {
    fn en(self) -> &'static str {
        use Key::*;
        match self {
            // Shared
            Untitled => "(untitled)",
            NewPasswordPlaceholder => "New master password",
            ValidationMinLength => "password must be at least {n} characters",
            PasswordsMismatch => "passwords don't match \u{2014} try again",
            ErrWriteSaltPrefix => "failed to write salt: ",
            ErrWriteKeyFilePrefix => "failed to write key file: ",
            ErrReadNotesPrefix => "failed to read notes: ",
            ErrDeriveRecovery => "failed to derive recovery key",

            // Lock
            Tagline => "A knot only you can untie.",
            LockMasterPasswordLabel => "Master password:",
            LockPasswordPlaceholder => "Enter master password, press Enter to unlock",
            Locked => "Locked.",
            LockedErrorPrefix => "Locked \u{2014} ",
            TooManyAttempts => "Too many attempts \u{2014} try again in {n}s",
            ForgotPassword => "Forgot password? Use your recovery key",
            ConfigUnavailable => "config directory unavailable",
            LockWrongPassword => "wrong master password",
            LockKeyMismatch => "vault key mismatch (corrupted or partial restore)",
            ErrReadKeyFilePrefix => "failed to read key file: ",
            ErrVaultPrefix => "vault error: ",

            // Setup
            SetupDescription => "Create a master password for your vault.",
            SetupHint => {
                "At least {n} characters. You'll get a recovery key next in case you forget it."
            }
            ConfirmPasswordPlaceholder => "Confirm password",
            SetupChoosePrompt => "Choose a master password and press Enter.",
            SetupConfirmPrompt => "Re-enter the same password to confirm.",
            ConfirmFirstSetup => "enter your password above first",
            ErrWriteRecoveryPrefix => "failed to write recovery file: ",
            ErrCreateVaultPrefix => "failed to create vault: ",
            ErrInitVaultPrefix => "failed to initialize vault: ",

            // Recovery reveal
            RecoveryRevealTitle => "Your recovery key",
            RecoveryRevealDescription => {
                "If you forget your password, these 12 words are the only way back into your \
                 vault. Write them down and store them somewhere safe \u{2014} they're shown \
                 only once."
            }
            RecoveryRevealWarning => {
                "Anyone who has these words can open your vault. Never share them or store them \
                 with your password."
            }
            RecoveryRevealDone => "I've saved it \u{2014} open my vault",

            // Recovery screen
            RecoveryTitle => "Recover your vault",
            RecoveryDescription => "Enter your 12-word recovery key and choose a new password.",
            RecoveryKeyPlaceholder => "word1 word2 word3 \u{2026} (12 words, separated by spaces)",
            RecoveryConfirmPlaceholder => "Confirm new password",
            RecoveryPrompt => "Type your recovery key and a new password, then press Enter.",
            RecoveryBack => "Back to unlock",
            ConfirmFirstRecovery => "enter your new password above first",
            RecoveryWordCount => "recovery key must be {n} words",
            RecoveryKeyInvalid => "invalid recovery key",
            RecoveryNotSetUp => "no recovery key set up for this vault",
            VaultDataCorrupted => "vault data corrupted (auth failed)",
            ErrOpenVaultPrefix => "failed to open vault: ",

            // Editor
            EditorImageBtn => "Image",
            EditorExportBtn => "Export",
            EditorLockBtn => "Lock",
            EditorPreviewShow => "Preview",
            EditorPreviewHide => "Hide preview",
            EditorTitlePlaceholder => "Title",
            EditorBodyPlaceholder => "Start writing\u{2026}",
            EditorEditing => "Editing: {title}",
            EditorNoNoteSelected => "No note selected.",
            EditorNoNoteSelectedHint => "No note selected \u{2014} click + New to start.",
            BacklinksTitle => "Backlinks",
            DialogInsertImage => "Insert image",
            DialogExportNote => "Export note",
            ErrReadImagePrefix => "Couldn't read that image: ",
            ErrImageUnsupported => "That image isn't a supported format (only PNG/JPEG).",
            ErrImageStore => "Couldn't store that image.",
            ErrExportPrefix => "Export failed: ",

            // Editor formatting toolbar
            ToolbarHeading => "Heading",
            ToolbarBold => "Bold",
            ToolbarItalic => "Italic",
            ToolbarCode => "Code",
            ToolbarQuote => "Quote",
            ToolbarList => "List",
            ToolbarLink => "Link",

            // Sidebar
            SidebarImport => "Import",
            SidebarNewNote => "+ New",
            SidebarSearchPlaceholder => "Search\u{2026}",
            SidebarSort => "Sort",
            SidebarSettings => "\u{2699} Settings",
            SidebarClear => "Clear",
            SidebarNoNotesYet => "No notes yet. Click + New.",
            SidebarNoMatchSearchTags => "No notes match your search and the selected tags.",
            SidebarNoMatchSearch => "No notes match your search.",
            SidebarNoMatchTags => "No notes match the selected tags.",
            DialogImportNote => "Import note",
            ErrImportTooLarge => "That file is too large to import (max {n} MiB).",
            ErrReadFilePrefix => "Couldn't read that file: ",
            ErrDeleteNotePrefix => "Couldn't delete note: ",

            // Sort labels
            SortCreated => "Created",
            SortTitleAsc => "A\u{2013}Z",
            SortTitleDesc => "Z\u{2013}A",

            // Settings modal
            SettingsTitle => "Settings",
            SettingsTheme => "Theme",
            SettingsThemeLight => "Light",
            SettingsThemeDark => "Dark",
            SettingsThemeSystem => "System",
            SettingsFontSize => "Font size",
            SettingsFontSmall => "Small",
            SettingsFontMedium => "Medium",
            SettingsFontLarge => "Large",
            SettingsAutoLock => "Auto-lock",
            AutoLockOff => "Off",
            AutoLockOneMinute => "1 min",
            AutoLockFiveMinutes => "5 min",
            AutoLockFifteenMinutes => "15 min",
            SettingsLanguage => "Language",
            LanguageJa => "\u{65e5}\u{672c}\u{8a9e}", // 日本語
            LanguageEn => "English",
            LanguageSystem => "System",
            SettingsDone => "Done",

            // Preview
            PreviewEmpty => "Nothing to preview yet.",
            PreviewImageUnavailable => "[image unavailable]",
            PreviewExternalImage => "[external image: {label}]",

            // Tag editor
            TagsAddPlaceholder => "Add tags\u{2026}",

            // Change password / Security
            SettingsSecurity => "Security",
            SettingsChangePassword => "Change master password",
            ChangePasswordTitle => "Change master password",
            ChangePasswordDescription => {
                "Enter your current password, then choose a new one. Your notes aren't \
                 re-encrypted \u{2014} only the password that unlocks them changes."
            }
            ChangePasswordCurrentPlaceholder => "Current password",
            ChangePasswordPrompt => "Enter your current password and a new one, then press Enter.",
            ChangePasswordCurrentWrong => "current password is incorrect",
            ChangePasswordEnterCurrentFirst => "enter your current password above first",
            ChangePasswordSuccess => "Password changed. Use the new password next time you unlock.",
            ChangePasswordCancel => "Cancel",

            // Backup & restore
            SettingsBackup => "Backup",
            SettingsBackupBtn => "Backup & restore\u{2026}",
            BackupTitle => "Backup & restore",
            BackupDescription => {
                "Save an encrypted snapshot of your whole vault to one file, or \
                 restore an earlier one. Backups are encrypted just like your \
                 vault, so they're safe to keep anywhere."
            }
            BackupFolderLabel => "Backup folder:",
            BackupFolderDefault => "(default location)",
            BackupChangeFolder => "Change folder\u{2026}",
            BackupRetentionLabel => "Backups to keep:",
            BackupNowBtn => "Back up now",
            BackupRestoreBtn => "Restore from a backup\u{2026}",
            BackupSuccess => "Backup saved to {path}",
            ErrBackupPrefix => "Backup failed: ",
            ErrRestorePrefix => "Restore failed: ",
            RestoreConfirmTitle => "Restore this backup?",
            RestoreConfirmBody => {
                "This replaces your current vault with the chosen backup and \
                 returns you to the lock screen to unlock it. Unsaved changes in \
                 this session are discarded."
            }
            RestoreConfirmBtn => "Restore & lock",
            RestoreCancel => "Cancel",
            DialogRestoreBackup => "Choose a backup to restore",
            BackupAutoLabel => "Automatic backup:",
            AutoBackupOff => "Off",
            AutoBackupDaily => "Daily",
            AutoBackupWeekly => "Weekly",

            // main
            ErrSaveChangesPrefix => "Couldn't save changes: ",
        }
    }

    fn ja(self) -> &'static str {
        use Key::*;
        match self {
            // Shared
            Untitled => "（無題）",
            NewPasswordPlaceholder => "新しいマスターパスワード",
            ValidationMinLength => "パスワードは{n}文字以上必要です",
            PasswordsMismatch => "パスワードが一致しません \u{2014} もう一度入力してください",
            ErrWriteSaltPrefix => "ソルトの書き込みに失敗しました: ",
            ErrWriteKeyFilePrefix => "鍵ファイルの書き込みに失敗しました: ",
            ErrReadNotesPrefix => "ノートの読み込みに失敗しました: ",
            ErrDeriveRecovery => "リカバリーキーの導出に失敗しました",

            // Lock
            Tagline => "自分だけが解ける結び目。",
            LockMasterPasswordLabel => "マスターパスワード:",
            LockPasswordPlaceholder => "マスターパスワードを入力し、Enter で解錠",
            Locked => "ロックされています。",
            LockedErrorPrefix => "ロック中 \u{2014} ",
            TooManyAttempts => "試行回数が上限に達しました \u{2014} {n}秒後に再試行できます",
            ForgotPassword => "パスワードをお忘れですか？ リカバリーキーを使う",
            ConfigUnavailable => "設定ディレクトリを利用できません",
            LockWrongPassword => "マスターパスワードが違います",
            LockKeyMismatch => "ボールトの鍵が一致しません（破損または不完全な復元）",
            ErrReadKeyFilePrefix => "鍵ファイルの読み込みに失敗しました: ",
            ErrVaultPrefix => "ボールトエラー: ",

            // Setup
            SetupDescription => "ボールトのマスターパスワードを作成してください。",
            SetupHint => {
                "{n}文字以上。忘れた場合に備えて、次の画面でリカバリーキーをお渡しします。"
            }
            ConfirmPasswordPlaceholder => "パスワードを確認",
            SetupChoosePrompt => "マスターパスワードを決めて Enter を押してください。",
            SetupConfirmPrompt => "確認のため同じパスワードをもう一度入力してください。",
            ConfirmFirstSetup => "先に上のパスワードを入力してください",
            ErrWriteRecoveryPrefix => "リカバリーファイルの書き込みに失敗しました: ",
            ErrCreateVaultPrefix => "ボールトの作成に失敗しました: ",
            ErrInitVaultPrefix => "ボールトの初期化に失敗しました: ",

            // Recovery reveal
            RecoveryRevealTitle => "リカバリーキー",
            RecoveryRevealDescription => {
                "パスワードを忘れた場合、この12単語だけがボールトに戻る唯一の方法です。\
                 書き写して安全な場所に保管してください \u{2014} 表示は一度だけです。"
            }
            RecoveryRevealWarning => {
                "この単語を持つ人は誰でもボールトを開けます。共有したり、\
                 パスワードと一緒に保管したりしないでください。"
            }
            RecoveryRevealDone => "保存しました \u{2014} ボールトを開く",

            // Recovery screen
            RecoveryTitle => "ボールトを復元",
            RecoveryDescription => {
                "12単語のリカバリーキーを入力し、新しいパスワードを設定してください。"
            }
            RecoveryKeyPlaceholder => "word1 word2 word3 \u{2026}（12単語、スペース区切り）",
            RecoveryConfirmPlaceholder => "新しいパスワードを確認",
            RecoveryPrompt => "リカバリーキーと新しいパスワードを入力し、Enter を押してください。",
            RecoveryBack => "解錠画面に戻る",
            ConfirmFirstRecovery => "先に上の新しいパスワードを入力してください",
            RecoveryWordCount => "リカバリーキーは{n}単語です",
            RecoveryKeyInvalid => "リカバリーキーが正しくありません",
            RecoveryNotSetUp => "このボールトにはリカバリーキーが設定されていません",
            VaultDataCorrupted => "ボールトのデータが破損しています（認証に失敗）",
            ErrOpenVaultPrefix => "ボールトを開けませんでした: ",

            // Editor
            EditorImageBtn => "画像",
            EditorExportBtn => "エクスポート",
            EditorLockBtn => "ロック",
            EditorPreviewShow => "プレビュー",
            EditorPreviewHide => "プレビューを隠す",
            EditorTitlePlaceholder => "タイトル",
            EditorBodyPlaceholder => "書き始める\u{2026}",
            EditorEditing => "編集中: {title}",
            EditorNoNoteSelected => "ノートが選択されていません。",
            EditorNoNoteSelectedHint => {
                "ノートが選択されていません \u{2014} ＋新規 で作成できます。"
            }
            BacklinksTitle => "リンク元",
            DialogInsertImage => "画像を挿入",
            DialogExportNote => "ノートをエクスポート",
            ErrReadImagePrefix => "その画像を読み込めませんでした: ",
            ErrImageUnsupported => "対応していない画像形式です（PNG / JPEG のみ）。",
            ErrImageStore => "その画像を保存できませんでした。",
            ErrExportPrefix => "エクスポートに失敗しました: ",

            // Editor formatting toolbar
            ToolbarHeading => "見出し",
            ToolbarBold => "太字",
            ToolbarItalic => "斜体",
            ToolbarCode => "コード",
            ToolbarQuote => "引用",
            ToolbarList => "リスト",
            ToolbarLink => "リンク",

            // Sidebar
            SidebarImport => "インポート",
            SidebarNewNote => "＋新規",
            SidebarSearchPlaceholder => "検索\u{2026}",
            SidebarSort => "並び替え",
            SidebarSettings => "\u{2699} 設定",
            SidebarClear => "クリア",
            SidebarNoNotesYet => "ノートがありません。＋新規 で作成。",
            SidebarNoMatchSearchTags => "検索条件と選択中のタグに一致するノートはありません。",
            SidebarNoMatchSearch => "検索条件に一致するノートはありません。",
            SidebarNoMatchTags => "選択中のタグに一致するノートはありません。",
            DialogImportNote => "ノートをインポート",
            ErrImportTooLarge => "そのファイルはインポートするには大きすぎます（最大{n} MiB）。",
            ErrReadFilePrefix => "そのファイルを読み込めませんでした: ",
            ErrDeleteNotePrefix => "ノートを削除できませんでした: ",

            // Sort labels
            SortCreated => "作成順",
            SortTitleAsc => "A\u{2013}Z",
            SortTitleDesc => "Z\u{2013}A",

            // Settings modal
            SettingsTitle => "設定",
            SettingsTheme => "テーマ",
            SettingsThemeLight => "ライト",
            SettingsThemeDark => "ダーク",
            SettingsThemeSystem => "システム",
            SettingsFontSize => "フォントサイズ",
            SettingsFontSmall => "小",
            SettingsFontMedium => "中",
            SettingsFontLarge => "大",
            SettingsAutoLock => "自動ロック",
            AutoLockOff => "オフ",
            AutoLockOneMinute => "1分",
            AutoLockFiveMinutes => "5分",
            AutoLockFifteenMinutes => "15分",
            SettingsLanguage => "言語",
            LanguageJa => "\u{65e5}\u{672c}\u{8a9e}", // 日本語
            LanguageEn => "English",
            LanguageSystem => "システム",
            SettingsDone => "完了",

            // Preview
            PreviewEmpty => "プレビューする内容がありません。",
            PreviewImageUnavailable => "[画像を表示できません]",
            PreviewExternalImage => "[外部画像: {label}]",

            // Tag editor
            TagsAddPlaceholder => "タグを追加\u{2026}",

            // Change password / Security
            SettingsSecurity => "セキュリティ",
            SettingsChangePassword => "マスターパスワードを変更",
            ChangePasswordTitle => "マスターパスワードを変更",
            ChangePasswordDescription => {
                "現在のパスワードを入力し、新しいパスワードを設定してください。\
                 ノートは再暗号化されません \u{2014} 解錠に使うパスワードだけが変わります。"
            }
            ChangePasswordCurrentPlaceholder => "現在のパスワード",
            ChangePasswordPrompt => {
                "現在のパスワードと新しいパスワードを入力し、Enter を押してください。"
            }
            ChangePasswordCurrentWrong => "現在のパスワードが違います",
            ChangePasswordEnterCurrentFirst => "先に上の現在のパスワードを入力してください",
            ChangePasswordSuccess => {
                "パスワードを変更しました。次回の解錠から新しいパスワードを使用してください。"
            }
            ChangePasswordCancel => "キャンセル",

            // Backup & restore
            SettingsBackup => "バックアップ",
            SettingsBackupBtn => "バックアップと復元\u{2026}",
            BackupTitle => "バックアップと復元",
            BackupDescription => {
                "ボールト全体を暗号化したスナップショットを1ファイルに保存したり、\
                 以前のものから復元できます。バックアップはボールトと同じく暗号化\
                 されているので、どこに保管しても安全です。"
            }
            BackupFolderLabel => "バックアップ先フォルダ:",
            BackupFolderDefault => "（既定の場所）",
            BackupChangeFolder => "フォルダを変更\u{2026}",
            BackupRetentionLabel => "保持するバックアップ数:",
            BackupNowBtn => "今すぐバックアップ",
            BackupRestoreBtn => "バックアップから復元\u{2026}",
            BackupSuccess => "バックアップを保存しました: {path}",
            ErrBackupPrefix => "バックアップに失敗しました: ",
            ErrRestorePrefix => "復元に失敗しました: ",
            RestoreConfirmTitle => "このバックアップから復元しますか?",
            RestoreConfirmBody => {
                "現在のボールトを選択したバックアップで置き換え、ロック画面に戻って\
                 復元したボールトを解錠します。このセッションの未保存の変更は破棄\
                 されます。"
            }
            RestoreConfirmBtn => "復元してロック",
            RestoreCancel => "キャンセル",
            DialogRestoreBackup => "復元するバックアップを選択",
            BackupAutoLabel => "自動バックアップ:",
            AutoBackupOff => "オフ",
            AutoBackupDaily => "毎日",
            AutoBackupWeekly => "毎週",

            // main
            ErrSaveChangesPrefix => "変更を保存できませんでした: ",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn language_round_trips_through_json() {
        for (lang, json) in [
            (Language::Ja, r#""ja""#),
            (Language::En, r#""en""#),
            (Language::System, r#""system""#),
        ] {
            assert_eq!(serde_json::to_string(&lang).unwrap(), json);
            let back: Language = serde_json::from_str(json).unwrap();
            assert_eq!(back, lang);
        }
    }

    #[test]
    fn language_default_is_system() {
        assert_eq!(Language::default(), Language::System);
    }

    #[test]
    fn both_languages_are_non_empty_for_every_key() {
        // Exercise both arms of every key so a stray `""` (or a copy-paste
        // that left an arm blank) is caught. The match arms are exhaustive,
        // so this also guarantees no key is missing a translation.
        for key in ALL_KEYS {
            assert!(!key.en().is_empty(), "empty en for a key");
            assert!(!key.ja().is_empty(), "empty ja for a key");
        }
    }

    #[test]
    fn parameter_tokens_present_in_both_languages() {
        // Parameterized keys must keep their substitution token in *both*
        // languages, or a translated string would silently drop the value.
        for (key, token) in [
            (Key::ValidationMinLength, "{n}"),
            (Key::TooManyAttempts, "{n}"),
            (Key::SetupHint, "{n}"),
            (Key::RecoveryWordCount, "{n}"),
            (Key::EditorEditing, "{title}"),
            (Key::PreviewExternalImage, "{label}"),
        ] {
            assert!(key.en().contains(token), "en missing {token}");
            assert!(key.ja().contains(token), "ja missing {token}");
        }
    }

    /// Every [`Key`] variant, for the table tests above. Kept in sync with
    /// the enum by hand; the `both_languages_are_non_empty_for_every_key`
    /// test would still pass if one were missing here, but the enum's
    /// exhaustive `match` is the real completeness guarantee.
    const ALL_KEYS: &[Key] = &[
        Key::Untitled,
        Key::NewPasswordPlaceholder,
        Key::ValidationMinLength,
        Key::PasswordsMismatch,
        Key::ErrWriteSaltPrefix,
        Key::ErrWriteKeyFilePrefix,
        Key::ErrReadNotesPrefix,
        Key::ErrDeriveRecovery,
        Key::Tagline,
        Key::LockMasterPasswordLabel,
        Key::LockPasswordPlaceholder,
        Key::Locked,
        Key::LockedErrorPrefix,
        Key::TooManyAttempts,
        Key::ForgotPassword,
        Key::ConfigUnavailable,
        Key::LockWrongPassword,
        Key::LockKeyMismatch,
        Key::ErrReadKeyFilePrefix,
        Key::ErrVaultPrefix,
        Key::SetupDescription,
        Key::SetupHint,
        Key::ConfirmPasswordPlaceholder,
        Key::SetupChoosePrompt,
        Key::SetupConfirmPrompt,
        Key::ConfirmFirstSetup,
        Key::ErrWriteRecoveryPrefix,
        Key::ErrCreateVaultPrefix,
        Key::ErrInitVaultPrefix,
        Key::RecoveryRevealTitle,
        Key::RecoveryRevealDescription,
        Key::RecoveryRevealWarning,
        Key::RecoveryRevealDone,
        Key::RecoveryTitle,
        Key::RecoveryDescription,
        Key::RecoveryKeyPlaceholder,
        Key::RecoveryConfirmPlaceholder,
        Key::RecoveryPrompt,
        Key::RecoveryBack,
        Key::ConfirmFirstRecovery,
        Key::RecoveryWordCount,
        Key::RecoveryKeyInvalid,
        Key::RecoveryNotSetUp,
        Key::VaultDataCorrupted,
        Key::ErrOpenVaultPrefix,
        Key::EditorImageBtn,
        Key::EditorExportBtn,
        Key::EditorLockBtn,
        Key::EditorPreviewShow,
        Key::EditorPreviewHide,
        Key::EditorTitlePlaceholder,
        Key::EditorBodyPlaceholder,
        Key::EditorEditing,
        Key::EditorNoNoteSelected,
        Key::EditorNoNoteSelectedHint,
        Key::BacklinksTitle,
        Key::DialogInsertImage,
        Key::DialogExportNote,
        Key::ErrReadImagePrefix,
        Key::ErrImageUnsupported,
        Key::ErrImageStore,
        Key::ErrExportPrefix,
        Key::SidebarImport,
        Key::SidebarNewNote,
        Key::SidebarSearchPlaceholder,
        Key::SidebarSort,
        Key::SidebarSettings,
        Key::SidebarClear,
        Key::SidebarNoNotesYet,
        Key::SidebarNoMatchSearchTags,
        Key::SidebarNoMatchSearch,
        Key::SidebarNoMatchTags,
        Key::DialogImportNote,
        Key::ErrImportTooLarge,
        Key::ErrReadFilePrefix,
        Key::ErrDeleteNotePrefix,
        Key::SortCreated,
        Key::SortTitleAsc,
        Key::SortTitleDesc,
        Key::SettingsTitle,
        Key::SettingsTheme,
        Key::SettingsThemeLight,
        Key::SettingsThemeDark,
        Key::SettingsThemeSystem,
        Key::SettingsFontSize,
        Key::SettingsFontSmall,
        Key::SettingsFontMedium,
        Key::SettingsFontLarge,
        Key::SettingsAutoLock,
        Key::AutoLockOff,
        Key::AutoLockOneMinute,
        Key::AutoLockFiveMinutes,
        Key::AutoLockFifteenMinutes,
        Key::SettingsLanguage,
        Key::LanguageJa,
        Key::LanguageEn,
        Key::LanguageSystem,
        Key::SettingsDone,
        Key::PreviewEmpty,
        Key::PreviewImageUnavailable,
        Key::PreviewExternalImage,
        Key::TagsAddPlaceholder,
        Key::SettingsSecurity,
        Key::SettingsChangePassword,
        Key::ChangePasswordTitle,
        Key::ChangePasswordDescription,
        Key::ChangePasswordCurrentPlaceholder,
        Key::ChangePasswordPrompt,
        Key::ChangePasswordCurrentWrong,
        Key::ChangePasswordEnterCurrentFirst,
        Key::ChangePasswordSuccess,
        Key::ChangePasswordCancel,
        Key::ErrSaveChangesPrefix,
    ];
}
