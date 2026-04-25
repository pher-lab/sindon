# Shroud UI フレームワーク セキュリティ監査レポート

**日付:** 2026-04-25
**対象:** shroud UI フレームワーク (rust-ui-project)
**監査範囲:** shroud_security, shroud_reactive, shroud_widgets, shroud_render, shroud_platform, shroud_app の各クレート

---

## 目次

1. [Critical](#critical)
2. [High](#high)
3. [Medium](#medium)
4. [総合評価](#総合評価)

---

## Critical

### C-1. デモ用マスターパスワードがハードコードされている

- **ファイル:** `examples/password_manager/src/main.rs:45`
- **重大度:** Critical
- **説明:** ソースコードに平文のパスワード `"hunter2"` が直接記載されている。デモ用途とはいえ、リポジトリにコミットされると誰でも参照可能。
- **コード:**
  ```rust
  const DEMO_PASSWORD: &str = "hunter2";
  ```
- **改善案:** 環境変数から読み込むか、デモビルドフラグ付きのランダム生成に変更する。

---

### C-2. クリップボード読み込み時に中間の String がメモリに露出する

- **ファイル:** `crates/shroud_platform/src/clipboard.rs:63-80`
- **重大度:** Critical
- **説明:** `read_secure()` は OS クリップボードから `String` を取得した後、`SecureString` にコピーする。この間、平文が `String` としてヒープ上に存在する。`String` は `ZeroizeOnDrop` を実装していないため、ドロップ時にゼロ化されない。
- **コード:**
  ```rust
  pub fn read_secure(&self) -> Result<SecureString, ClipboardError> {
      let text = board.get_text().map_err(|_| ClipboardError::ReadFailed)?;
      let secure = SecureString::new(&text);
      // ... zeroize attempt follows
  }
  ```
- **改善案:** `arboard` の代わりにクリップボードに直接書き込む API を探すか、`zeroize` クレートで中間バッファを明示的にゼロ化し、`std::mem::forget` でデストラクタをスキップする。

---

### C-3. `write_volatile` によるゼロ化は信頼できない

- **ファイル:** `crates/shroud_platform/src/clipboard.rs:72-77`
- **重大度:** Critical
- **説明:** `std::ptr::write_volatile` はメモリバリアのみを保証し、コンパイラによる最適化からの保護はしない。コンパイラは volatile 書き込みを削除・リオーダーする可能性があるため、機密データの完全な消去が保証されない。
- **コード:**
  ```rust
  unsafe {
      let bytes = text.as_bytes_mut();
      for b in bytes.iter_mut() {
          std::ptr::write_volatile(b, 0);
      }
  }
  ```
- **改善案:** `zeroize::Zeroize::zeroize()` を使用するか、`memsec` crate のような OS レベルの保護機能を利用する。

---

## High

### H-1. `SecureString`/`SecureBuffer` が内部に `String`/`Vec<u8>` を使用

- **ファイル:** `crates/shroud_security/src/secure_string.rs:13-15`, `crates/shroud_security/src/secure_buffer.rs:11-13`
- **重大度:** High
- **説明:** `SecureString` は `String` を、`SecureBuffer` は `Vec<u8>` を内部に保持している。`String::from()` や `Vec::push()` の実行時に、古いバッファを解放する際にメモリ上に一時的に機密データが残る可能性がある。`zeroize` は現在のバッファのみをゼロ化し、解放された古いバッファは残る。
- **コード:**
  ```rust
  pub struct SecureString { inner: String }
  pub struct SecureBuffer { inner: Vec<u8> }
  ```
- **改善案:** `mlock` 済みメモリに直接書き込むカスタムアロケータを使用する、または `zeroize` crate の `Zeroizing` 型と組み合わせる。

---

### H-2. 機密データが一般 `Signal<T>` に格納可能（コンパイル時強制なし）

- **ファイル:** `crates/shroud_reactive/src/signal.rs`
- **重大度:** High
- **説明:** `SecureSignal<T>` が mlock'd arena に機密データを保存する仕組みを提供しているが、`Signal<String>` でパスワード等を扱うことをコンパイル時に防止する仕組みがない。開発者が誤って一般信号に機密データを入れると、mlock'd arena の保護が適用されない。
- **改善案:** 機密データ型を受け取る信号を `SecureSignal` のみに制限する型レベルの強制を追加する。

---

### H-3. `Signal::get_clone()` が機密データのプレーンなクローンを返す

- **ファイル:** `crates/shroud_reactive/src/signal.rs:72-74`
- **重大度:** High
- **説明:** `Signal<String>` の `get_clone()` を呼び出すと、ヒープ上に平文のクローンが作成される。このクローンは `SecureString` でないため、ドロップ時にゼロ化されない。
- **コード:**
  ```rust
  pub fn get_clone(&self) -> T {
      self.with(|v| v.clone())
  }
  ```
- **改善案:** 機密データを含む信号には `get_clone()` を提供せず、`expose()` クロージャー経由のアクセスのみを許可する。

---

### H-4. 非セキュア `Input` ウィジェットが `String` をゼロ化せずに保持

- **ファイル:** `crates/shroud_widgets/src/input.rs:41`
- **重大度:** High
- **説明:** 通常の `Input` ウィジェットは `RefCell<String>` を内部に保持している。ウィジェットがドロップされても、`String` はデフォルトのデストラクタで解放されるのみで、メモリの内容はゼロ化されない。
- **コード:**
  ```rust
  value: RefCell<String>,
  ```
- **改善案:** 機密データを含む可能性がある入力には `SecureInput` の使用を強制するドキュメントまたは型レベルのチェックを追加する。

---

### H-5. `Reactive::Dynamic` のクロージャーが機密データをキャプチャ可能

- **ファイル:** `crates/shroud_reactive/src/reactive.rs:49`
- **重大度:** High
- **説明:** `Reactive::derive()` のクロージャーが機密データをキャプチャでき、保護パスを通らない。クロージャー内で機密データにアクセスすると、保護されていないメモリ上に露出する。
- **コード:**
  ```rust
  Dynamic(Rc<dyn Fn() -> T>),
  ```
- **改善案:** 機密データを含むクロージャーには `SecureReactive` のような型を提供する。

---

## Medium

### M-1. セキュアアリーナの最大アロケーションが 4 KB

- **ファイル:** `crates/shroud_security/src/arena.rs:12`
- **重大度:** Medium
- **説明:** サイズクラスが最大 4 KB まで。4 KB を超える機密データはアロケーションに失敗する。
- **コード:**
  ```rust
  const SIZE_CLASSES: [usize; 4] = [64, 256, 1024, 4096];
  ```
- **改善案:** より大きなサイズクラスを追加するか、動的にサイズクラスを拡張する仕組みを検討する。

---

### M-2. OS クリップボードへの書き出しはフレームワークの制御外

- **ファイル:** `crates/shroud_platform/src/clipboard.rs`
- **重大度:** Medium
- **説明:** `SecureClipboard::write_secure()` は OS クリップボードに平文を書き出す。OS 側での永続化・共有はフレームワークが制御できない。10 秒の自動クリアはフレームワーク側のタイマーであり、OS が既にクリップボード内容を保存した場合は効果がない。
- **改善案:** ユーザーにクリップボード使用のリスクを明示する警告を表示する。

---

### M-3. Linux/macOS で画面キャプチャ防止が未実装

- **ファイル:** `crates/shroud_platform/src/display_protection.rs`
- **重大度:** Medium
- **説明:** Linux は `Unsupported` のみ。macOS も objc2 統合待ち。Windows のみの対応。
- **改善案:** Linux (X11/Wayland) での代替対策（`WL_SHM_BUFFER_EXPORT` など）を調査・実装する。

---

### M-4. セキュアアリーナがスレッドローカル

- **ファイル:** `crates/shroud_reactive/src/secure_signal.rs:11-13`
- **重大度:** Medium
- **説明:** スレッド間で機密データが渡されるとセキュア領域から外れる可能性がある。
- **コード:**
  ```rust
  thread_local! {
      static SECURE_ARENA: OnceCell<SecureArena> = const { OnceCell::new() };
  }
  ```
- **改善案:** スレッド間で機密データを安全に渡す仕組み（シリアライズ + セキュア送信）を検討する。

---

## 総合評価

| 重大度 | 件数 | ステータス |
|--------|------|-----------|
| Critical | 3 | 即座に対応が必要 |
| High | 5 | 短期対応を推奨 |
| Medium | 4 | 計画的に対応 |

### 優先対応リスト

1. **[Critical]** クリップボード `read_secure` のゼロ化を `zeroize` クレートに置き換える
2. **[Critical]** ハードコードされたデモパスワードを削除または環境変数化
3. **[Critical]** `write_volatile` の脆弱なゼロ化を `zeroize` crate に置き換える
4. **[High]** 機密データ型を `SecureSignal` のみに制限する型レベルの強制
5. **[High]** `SecureString`/`SecureBuffer` の内部表現を `mlock` 済みメモリに直接割り当て
6. **[High]** `Signal::get_clone()` の機密データ漏洩リスクをドキュメント化
7. **[Medium]** Linux/macOS での画面キャプチャ防止を実装
8. **[Medium]** より大きな機密データのアロケーションをサポート
