# Knot dogfood log

> **目的**: Knot を実際の毎日のメモ帳として使い、ロードマップが予測できない *現実のバグ* と *本当に欲しい UX* を炙り出す。pre-1.0 (リリース判断 E) の前段活動。
>
> **使い方**: 使ってて「ん?」と思ったら、下の Inbox に1行で放り込むだけ。整形は後回し。週次でトリアージして、本物だけを roadmap / issue に昇格させる。
>
> **凡例**: `🐞 bug` / `🧷 friction (動くけど面倒)` / `💡 idea` / `🔒 security smell`
> **重要度**: `P1 毎日刺さる` / `P2 たまに` / `P3 あれば嬉しい`

実行バイナリ: `target/release/knot.exe`(最新コードで焼き直したもの)
保存先 vault: (初回起動時にメモ)

---

## Inbox(未トリアージ — ここに雑に足す)

<!-- 例:
- 2026-06-18 🐞 P1 — find/replace バーを Esc で閉じても caret が本文に戻らない
- 2026-06-18 🧷 P2 — note 切替時に scroll 位置が頭に戻ってほしくない時がある
- 2026-06-18 💡 P3 — pin したノートを sidebar 最上部で区切り線で分けたい
-->

(2026-06-18 の初回バッチ22件 + 2026-06-21 の6件はトリアージ済みへ移動)

---

## トリアージ済み

> 2026-06-18 初回トリアージ。`[fw]` = shroud framework 側 / `[app]` = Knot 側。
> framework 項目が**リリース対象本体の穴**なので最優先。P1 2件 (FW-1/FW-2) は実コードで真因確定済み。

### → 昇格(roadmap / 実装行き)

#### framework (shroud) — リリース前に効く本丸

- **FW-1 [fw] P1 — IME 未確定文字列(preedit)が表示されない** (元 #25, #24)
  真因確定: [event_loop.rs](../crates/shroud_app/src/event_loop.rs) が `Ime::Preedit` を明示的に無視 (`Commit` のみ処理)。winit は `set_ime_allowed(true)` 時にインライン変換文字列を OS に描かせず **アプリに描画させる** 設計なので、無視 = 確定まで何も見えない。コメントの「OS が変換窓を出す」前提は誤り。
  - **✅ #25 = 完了**: `WidgetEvent::ImePreedit` 新設 + pure `translate_ime` + Input が preedit を表示専用スプライス→下線描画 (commit まで `value` 不変、FocusLost で破棄)。candidate-window が下線に被る件も IME cursor area を行底+2px まで伸ばして解消。widget 5 + translation 3 テスト緑、実機 OK。
  - **✅ #24 = 完了 (実機 OK)**: 真因 = `PlatformWindow::set_ime_allowed(true)` が Windows で `force_attach_ime_windows` → `ImmSetOpenStatus(himc, true)` を呼び、focus の度に IME を変換 ON(JP 配列＝ひらがな)へ強制。winit の `set_ime_allowed(true)` は IME を*関連付ける*だけで open status は触らないので、強制はこの1行が原因。対応(方針 A)= `ImmSetOpenStatus` 強制を撤去し open status を OS/ユーザーに委譲(`force_attach_ime_windows` + 未使用 `raw_window_handle` import 削除、`Focused(true)` コメント更新)。[[progress-phase39-ime-unblock]] で真犯人は `exploit_mitigation:true`(既に default false 化)と判明済 ∴ この hack は残骸だった。実機: ①ひらがな既定が解消 ②半角/全角 で日本語入力は退行なし ③SecureInput の IME suppression(Tier-2)維持。全テスト緑・fmt・clippy クリーン。
- **✅ FW-2 [fw] P1 — soft-wrap 折り返し後の ↑↓ 移動が壊れる (元 #26, #27, #29) = 完了**
  真因: ↑↓ が **hard line(段落)単位**で視覚的な折り返し行を見ていなかった + sticky column が文字 index 基準。さらに caret 描画が prefix シェイプで**折り返し境界で1行ズレる**既存バグ(クリックでも発症)も同根。
  対応: ①↑↓ を視覚行 + sticky-**x** ベースに、`event` は net 行数を貯めて `paint` で engine 解決(`pending_vmove`/`desired_x`)。#29 端ジャンプ同梱。②`shroud_text::caret_at_offset` 新設 — 全文シェイプで offset→(x,y) を出し折り返し境界の caret ズレを解消(`cursor_position` の prefix シェイプを置換)。vnav spike 4 + engine 2 テスト緑、既存 241+ 回帰なし、実機 OK。
- **✅ FW-3 [fw] P2 — 右端 caret がスクロールバーに被る (元 #34) = 完了**
  multiline の wrap 幅から `SCROLLBAR_LANE` (= `SCROLLBAR_WIDTH + SCROLLBAR_INSET` = 8px) を**常時**差し引き、本文・caret・ヒットテスト・選択をバー手前で止める(バー出入りで幅が揺れないよう常時予約)。viewport spike に「glyph がレーンに入らない」テスト追加、実機 OK。
- **✅ FW-4 [fw] P2 — color emoji が真っ白 (元 #28) = 完了 (実機 OK)**
  真因確定: swash は color emoji をフル RGBA で出していたが、`rasterize` が alpha だけ抜き取り → R8 atlas → text shader が `.r × text色(白)` を掛けて真っ白に。
  対応: ①`GlyphImage.is_color` 追加 + `rasterize` の `SwashContent::Color` で RGBA 保持(alpha 抜き取りをやめる)。②`TextureAtlas` を bytes-per-pixel+format 化(`new()`=R8 据置、`new_rgba()`=Rgba8UnormSrgb 追加、upload/clear が bpp 対応)。③renderer に RGBA `color_atlas` + bind group。upload を `is_color` で振り分け、`build_text_geometry` を atlas-membership で自動分割(color は白 tint で本来色を保持・text色の alpha だけ尊重)、描画は **image pipeline 流用**(`texel×tint`)で text の後に1パス。image と同じ sRGB + ALPHA_BLENDING 経路なので premultiply 不要。`SecureTextureAtlas` は R8 のまま無傷。回帰ガード(ASCII=mask 1bpp / emoji=RGBA 4bpp)。fmt/clippy/build/全テスト緑。実機で絵文字フルカラー表示・エッジズレ無しを確認。
- **✅ FW-5 [fw] P2 — 画像が荒い (元 #40) = 完了 (実機 OK)**
  真因確定: 画像テクスチャは `mip_level_count: 1`(原寸1枚)でアップロードされ、sampler に mipmap が無かった。ノートに貼る大きい写真/スクショをテキスト幅に縮小表示すると、bilinear minification が出力1pxあたり 2×2 texel しか平均しない → ジャギ/チラつき(no-mipmap minification aliasing)。nearest でも DPI でもない。
  対応: ①`shroud_render::image::build_mip_chain` 新設 — **premultiplied-linear** で 2×2 box 縮小を 1×1 まで反復生成(透過エッジの色ブリード防止 + GPU の sRGB sampling と輝度整合)。②`ensure_image_uploaded` で `mip_level_count` を確保し各レベルを `write_texture`(unique 画像ごと1回キャッシュ、mip pixels は upload 後 drop)。③共有 sampler を `mipmap_filter: Linear`(trilinear)に(1-mip の glyph atlas には無害)。画像シェーダは `textureSample`(自動 LOD)なので追加変更不要。mip 生成器に純関数テスト5件(寸法半減・非正方 clamp・solid 保存・premult 非ブリード)。fmt/clippy/build/全テスト緑。実機目視で荒さ解消を確認(FHD プレビューサイズでは縮小で小さくなるが minification の荒れは消えた)。

> 2026-06-21 第2バッチ(6件)トリアージ。**全件 framework (shroud) 側**で Input の選択 / focus ring / scroll の磨き。P1(release-blocker)は無く、全て P2/P3。FW-1〜5 は完了済 ∴ 採番は FW-6 から。

- **✅ FW-6 [fw] P2 — 選択が改行/行末で途切れて見える(元: 改行も選択ハイライト)= 完了**
  真因確定: [engine.rs](../crates/shroud_text/src/engine.rs) `selection_rects` は各 layout run で `run.highlight()`(= その行の*グリフ*が覆う pixel 区間のみ)を取り、`if w > 0.0` で 0 幅を捨てる。∴ 選択が次行へ続く行でも**行末の改行分にハイライトが出ない**ので、改行が選択に含まれているか視覚的に分からない。
  対応: `selection_rects` は pure のまま据え置き、内部実装を共有する新 API `selection_rects_with_trailing` を追加。選択がその視覚行の最終グリフより先(= 改行 / soft-wrap 継ぎ目)へ続く行だけ、行末に `font_size × 0.33` の trailing sliver を足す(空行も left edge に出して可視化)。caret 幾何 ([`cursor_position`]) と IME preedit 下線は pure 版を使い続けるので phantom sliver は出ない。Input は選択ハイライト描画のみ trailing 版へ切替。engine テスト3件(改行で付与 / 最終行は非付与 / 選択中の空行を可視化)。fmt/clippy/全テスト緑(shroud_text+shroud_widgets、241 widget test 回帰なし)。**実機 OK**(2026-06-22 ユーザー確認: 変化は小さいが改行込み選択がかなり分かりやすくなった)。
- **✅ FW-7 [fw] P2 — スムーススクロール (ScrollView) = 完了**
  ホイールで `scroll_y` が瞬間ジャンプ → 位置を見失う。対応 = 既存 B-8 animation ([[progress-animation-b8]]) を流用。primitive に `Animated::snap`(補間なしで即値・vote なし)を追加し、ScrollView の `scroll_y: f32` を `Animated<f32>`(lazy)へ置換。**wheel = `set`(120ms EaseOut で eased)/ system re-clamp = `snap`(即時、note 切替で reused viewport が不自然にスライドしない)**。`scroll_y()` は論理 target を返す(既存テスト互換)、paint / `scroll_offset`(ヒットテスト)/ scrollbar thumb は displayed (eased) 値。`scroll_transition(Duration::ZERO)` で旧・即時挙動へ opt-out 可。frame-vote pump (event→request_redraw→paint が get で vote→次フレーム) で駆動。reactive 1 + widget 2 テスト追加 + 既存 hit-test/paint 3件を ZERO 化で切り分け。fmt/clippy/全テスト緑(243 widget)。**実機 OK**(2026-06-22 ユーザー確認: 綺麗に決まり、迷わなくなった)。
- **✅ FW-7b [fw] P2 — スムーススクロール (Input 内部 viewport) = 完了**
  multiline Input の内部 `scroll_y: Cell` を `Animated<f32>`(FW-7 と同型)へ置換。**wheel = `set`(120ms EaseOut)/ caret-reveal・re-clamp・note 切替リセット = `snap`(即時)**。paint は eased `displayed` を hit-test / offset push / scrollbar に使い、target は wheel/reveal/clamp が操作。`Input::scroll_transition(Duration)` builder 追加(ScrollView と対称、`ZERO` で opt-out)。慎重所だった「typing 中の scroll-to-caret は即時」を snap で担保(wheel だけ滑らか)。viewport spike: 既存呼び出しを `ZERO` 化で決定的に + easing 検証テスト1件追加。fmt/clippy/全テスト緑(243 widget + reactive)。**実機 OK**(2026-06-22 ユーザー確認: ホイール滑らか・タイプ中の caret 即追従・note 切替で先頭即ジャンプ、全部期待通り)。
- **✅ FW-8 [fw] P3 — focus ring がクリックでも出る(:focus-visible 相当が無い)= 完了 (実機 OK)**
  真因確定: 各 widget が `if self.focused { ctx.paint_focus_ring(..) }` で focus 状態だけ見て ring を描き、focus の**理由(pointer / keyboard)を区別しない**。auto-focus-on-click でクリックでも focused → ring。対応: [focus.rs](../crates/shroud_widgets/src/focus.rs) に `FocusReason`(Pointer/Keyboard/Programmatic、`shows_ring()`=Pointer のみ false)+ `FocusManager.visible` フラグ。`tree.focus()` は内部 `focus_with_reason` に委譲し、クリック経路=Pointer(抑制)/ Tab=Keyboard / 公開 `focus()`=Programmatic(表示)。**public `tree.focus()` シグネチャ不変 ∴ knot/examples 配線ゼロ**。`tree.paint` が毎フレーム `ctx.set_focus_visible(...)` を発行 → 各 widget は `self.focused && ctx.focus_visible()` で gate。SecureInput の `suppress_ime()`(Tier-2)は focus だけで発火し ring とは独立に据え置き。同一 widget 再 focus でも visible を no-op return 前に更新 ∴ Tab→click で ring が消える。FW-8 テスト3件 + focus.rs reason 単体3件。
- **✅ FW-9 [fw] P3 — focus ring が四角で角丸でない = 完了 (実機 OK)**
  対応: rect SDF シェーダに **stroke(border)モード**追加([renderer.rs](../crates/shroud_render/src/renderer.rs) `DrawRect.border_width`)。outer SDF から `d+border_width` の inner fill を引いて両エッジ AA 付きの同心アウトラインを1枚 rect で描く(filled fast-path は `radius<=0 && border<=0` のときだけ)。`paint_focus_ring` を 4 本 sharp rect → **1枚の `stroke_rect_rounded`** に。widget radius を受け取り `radius + offset + width` で同心円化(四角 widget は四角維持)。Button/Dropdown=自前 radius、Input/Checkbox/SecureInput=0。既存 ring テスト6件を「4 rect → 1 stroke rect + border_width 検証」へ更新。`shroud_render`+`shroud_widgets` 全緑、workspace(knot 除く)ビルド警告ゼロ、fmt/clippy クリーン。**実機 OK**(2026-06-24 ユーザー確認: クリックで ring 出ない / Tab で角丸 ring、きれいに動作)。
- **✅ FW-10 [fw] P3 — triple-click で行選択 = 完了 (2026-06-27, 実機 OK)**
  真因確定: double-click 単語選択は実装済(`word_bounds`/`DOUBLE_CLICK_MAX`)だが、`last_click` が double 発火後に `None` reset され **triple が連鎖しない**設計だった。
  対応([input.rs](../crates/shroud_widgets/src/input.rs)): ①`last_click` を `(Instant, Point, u8)` 化してクリック回数を保持し、近接連打を `1→2→3→1…` と循環カウント(double 後の reset を撤去)。②`pending_word_select: Cell<bool>` を `pending_select: Cell<SelectUnit>`(`Caret`/`Word`/`Line`)へ一般化。③純関数 `line_bounds`(`\n` 区切りの**論理行**、末尾改行を除外 ∴ 選択置換で次行と繋がらない)を追加し、paint の deferred-hit 解決で count=2→`word_bounds` / count=3→`line_bounds` に分岐。④`FocusLost` reset も `pending_select` へ。
  設計判断: triple = **論理行(段落)**選択(視覚行ではなく `\n` 区切り)。ブラウザ/一般エディタの mental model に一致し、`line_bounds` を engine 非依存の純関数にできてテスト容易。**FW-11(日本語 word-select)の現実的 fallback も兼ねる**(行まるごと選択の安定ジェスチャ)。
  テスト: `line_bounds` 純関数テスト5件(中間行/先頭末尾/空行/単一行/multibyte 境界)+ 既存 `word_bounds` 9件 回帰なし。fmt/clippy(新規警告ゼロ)/全テスト緑。**実機 OK**(2026-06-27 ユーザー確認: 行選択 OK・double-click 単語選択退行なし・日本語行 OK)。
- **✅ FW-11 [fw] P3 — 日本語等の単語選択(double-click)を script-run 区切りに = 完了 (2026-07-04)**
  真因: [input.rs](../crates/shroud_widgets/src/input.rs) `classify` が `is_alphanumeric()` を使うため CJK(漢字/ひらがな/カタカナ)を全部 `Word` 扱い → double-click が **CJK の連続全体**(実質その文/行まるごと)を掴んでいた。正しい分かち書きは辞書(MeCab 級)必須で、zeroize-first の最小 framework には重い。
  対応(**方針 ① 辞書なし script-run** — 方向案②の triple-click 行 fallback は FW-10 で既に landed 済 ∴ 残っていた「①も足すか」の判断を①採用で消化): `CharClass` に `Han`/`Hiragana`/`Katakana` を追加し、`classify` を CJK スクリプト範囲(コードポイント判定 `is_hiragana`/`is_katakana`/`is_han` を `is_alphanumeric` の**前**に評価。CJK も alphanumeric なので順序が要)で分類。`word_bounds` は「同 class の連続を掴む」既存ロジックのまま ∴ double-click が**スクリプトの切れ目で止まる**(日本語 の漢字部だけ / 続くかなだけを掴む)。カタカナ長音 `ー`(U+30FC)や 佐々木 の `々`(U+3005)は各 script に含めて run を割らない。空白区切りの Latin/Hangul 等は従来どおり `Word` で space 境界まで(挙動不変)。
  設計判断: 真の word segmentation ではなく「多少マシ」な近似(辞書ゼロ・純関数・framework 追加最小)。ユーザー文言「使えないなら使えないで何かできないか」に対し、①(script-run)+②(triple-click 行、FW-10)の二段 fallback で応答。
  テスト: `word_bounds` に CJK 区切り3件追加(漢字↔かな境界 / カタカナ長音 `ー` を割らない / ひらがな↔カタカナ境界)+ 既存9件・`line_bounds` 5件 回帰なし。fmt / clippy(workspace 含 knot・knot_clone、新規警告ゼロ)/ 全 widget test(277)/ lib unit(word_bounds 12)/ rustdoc -D warnings すべて緑。**実機 OK**(2026-07-04 ユーザー確認: 漢字部/かな部が別々に選択・長音を割らない・triple-click 行選択と英単語 double-click に退行なし)。
- **✅ FW-12 [fw] P2 💡 — アイコン描画手段が無い(アイコンフォント / SVG)= 完了 (2026-06-27, 実機 OK)**
  調査で確定した現状: レンダラのプリミティブは `DrawRect`・`DrawGlyph`・`DrawImage`(RGBA8) の3種のみでベクター/SVG 経路が無く、アプリ同梱フォントをロードする一級 API も無かった(`font_system().db_mut().load_font_data()` の escape hatch のみ)。3案(① mono PNG を DrawImage / ② アイコンフォント同梱 API / ③ SVG ラスタ)から、既存 glyph atlas/tint/shape 経路を再利用できる **② を採用**(framework 追加が小さく zeroize 非干渉)。設計判断: フォント本体は **app が OSS フォントを同梱**(framework は load API のみ)、アイコンの **名前→コードポイント対応は app 側 helper**。
  対応(framework・小):
  - `TextEngine::load_font_data(&[u8]) -> Vec<String>`([engine.rs](../crates/shroud_text/src/engine.rs)) — fontdb へ登録 + 使えるようになった family 名を返す(呼び手が `Named` を組める DX)。登録後 shape cache を drop。
  - `App::font(impl Into<Cow<'static,[u8]>>)`([event_loop.rs](../crates/shroud_app/src/event_loop.rs)) — `resumed` の最初の paint 前に登録(`fonts_loaded` ガードで suspend/resume の二重登録回避)。
  - `Button::family(TextFamily)`([button.rs](../crates/shroud_widgets/src/button.rs)) — ラベルをアイコンファミリで shape(`shape_text`→`shape_text_attrs`、default attrs は従来と等価 ∴ 既存ボタン無影響)。アイコンボタンの最小フック。
  対応(Knot):
  - `assets/knot-icons.ttf` — MDI webfont(`@mdi/font@7.4.47`)を `pyftsubset` で **15グリフにサブセット(1.3MB→2.5KB)**、Apache 2.0([ICON-FONT-LICENSE.txt](../examples/knot/assets/ICON-FONT-LICENSE.txt) 同梱)。family 名 `Material Design Icons`。
  - [`icons.rs`](../examples/knot/src/icons.rs) — `Icon` enum→コードポイント + `icon_button()` helper。`main.rs` で `App::font(icons::FONT)`。
  - [`toolbar.rs`](../examples/knot/src/toolbar.rs) のフォーマットボタン7個を `Heading/Bold/見出し/太字…` テキスト → アイコングリフに置換。不要になった `Toolbar*` i18n キー削除。
  テスト/検証: shroud_text に load+名前解決テスト2件(fixture = サブセット font)、framework 全テスト回帰なし、fmt/clippy(framework+knot)新規警告ゼロ、起動スモーク OK。**実機 OK**(2026-06-27 ユーザー確認: 表示きれい・視認性 OK・light⇄dark 両方 OK)。
  - **カラー(COLR)アイコンは未対応のまま様子見**(モノクロ + tint が今回の対象。FW-4 の color atlas 経路に乗せれば多色も出る余地はあるが未着手)。
- **FW-13 [fw] P2 🧷💡 — ツールチップ(hover で説明を出す手段)が無い = ✅ 完了 (2026-06-27 起票・同日着地、実機 OK)**
  動機: Tauri 版 Knot と見比べると、FW-12 でアイコン化したツールバー等のアイコンボタンや省略ラベルに hover 説明が無く**不親切になりやすい**。
  **⚠ 起票時の当初案は誤り**: 「hover で `popover()` Layer を push、exit で pop」は**そのままでは動かない**。Layer はアクティブな間メイン tree の pointer イベントを全部奪う([tree.rs](../crates/shroud_widgets/src/tree.rs) `dispatch_event` が `layers.last()` を無条件にイベント対象化 / layer 外 MouseMove も握り潰す) → トリガーに **MouseLeave が届かず** tip が消えない/ちらつく。**tip には「描画されるが入力を奪わない click-through オーバーレイ」が必須**と判明。
  着地(framework 2点 + app):
  - **①`Container::on_hover_enter(FnMut(Rect, &mut EventContext))` / `on_hover_exit`** ([container.rs](../crates/shroud_widgets/src/container.rs)) — 内部 MouseEnter/Leave をアプリへ surface。enter は自身の layout rect を渡す → そのまま `AnchorRect`。hover コールバックを付けても **hover 背景フェードは点かない**(`hoverable` 限定維持)。
  - **②`LayerOptions::tooltip()` / `interactive` フラグ** ([layer.rs](../crates/shroud_widgets/src/layer.rs)) — click-through Layer。layout・paint は全 Layer を回す既存ループにそのまま乗り、**イベント対象選択だけ**を「最上位 *interactive* Layer」に変更(`dispatch_event` / Escape・outside-click dismiss / Tab traversal / push 時の hover クリアを全て interactive 限定化)。
  - **③app 配線** ([tooltip.rs](../examples/knot/src/tooltip.rs)) — thread-local controller + 既存 `on_frame` tick で ~400ms 遅延ポーリング → click-through tip を push、exit で pop。`tooltip::trigger(text)` で各ツールバーアイコンを包む。i18n キー7個(en/ja)。auto-lock で screen が消えた時用に `vault_screen::build` で reset。
  テスト/検証: framework に [tooltip_tests.rs](../crates/shroud_widgets/tests/tooltip_tests.rs) 6件(hover コールバック / click-through 性質 + interactive 対照 / Tab スキップ / no-highlight ガード / hover→leave E2E)、shroud_widgets 全テスト緑、fmt/clippy(framework+knot --all-targets)クリーン。**実機 OK**(2026-06-27 ユーザー確認: 出る・消える・ちらつかない・レイアウト不変)。commit `093ebfb`(framework)/ `6c67649`(knot)。
  - 将来: `Tooltip` widget としてラップ(現状は Container を `trigger()` で包む方式)/ 遅延精度は tick 粒度(500ms)依存なので細かくしたければ別途。
- **✅ FW-14 [fw] P3 🧷 — Input の見た目が「枠つき四角」固定でカスタムできない = 完了 (2026-06-29, 実機 OK)**
  動機: Input は角丸・borderless・枠色変更ができず、検索バー風 / 下線だけ / 枠なしインライン編集 といったバリエーションが作れない。デザインは二の次だが、アイコン化(FW-12)・ツールチップ(FW-13)で UI が整ってくると、四角い枠だけが浮きやすい。
  真因確定(2026-06-27 コード確認): [input.rs](../crates/shroud_widgets/src/input.rs) `paint` が `fill_rect` 直描きで **背景塗り + 全周 1px ボーダー(上下左右 4 本の sharp rect)** を固定で描いていた。角丸経路に乗っておらず、`border_color` フィールドはあるが pub builder が無く `resolve_border` が theme `input_border` を読むだけ ∴ 枠色すらインスタンス単位で変えられなかった。
  対応(framework): **FW-9 の角丸ストローク経路(`fill_rect_rounded` / `stroke_rect_rounded` = `DrawRect.border_width`)をそのまま流用**しレンダラ無改造。`paint` の border を **4 本の sharp rect → `fill_rect_rounded(bg, radius)` + (枠ありなら) 内側 1px の `stroke_rect_rounded(border, radius, 1.0)` 1 枚**に置換(SDF が角を radius に合わせて丸める)。builder 3 つを Container/Button と対称に追加: `.radius(px)`(塗り+枠を角丸、負値 0 クランプ)/ `.border_color(c)`(インスタンス単位の枠色)/ `.borderless()`(枠を消し塗りだけ)。**デフォルト不変(radius 0・枠あり)∴ 既存 Input は見た目そのまま**。paint テスト5件(rect 数=塗り+枠 / 両 rect の角丸 / 負値クランプ / borderless で枠消失 / 枠色上書き)。`shroud_widgets` 251 テスト緑、fmt/clippy(framework+knot --all-targets)クリーン。commit `2fda28a`。
  対応(Knot・初の実使用): [sidebar.rs](../examples/knot/src/sidebar.rs) の検索ボックスに `.radius(8.0)` を適用し「検索バー風」に。commit `9ea2362`。**実機 OK**(2026-06-29 ユーザー確認: 角丸きれい・入力/プレースホルダ/focus ring 退行なし・light/dark 両方 OK)。
  - 将来: `padding` の builder 化 / 下線だけ(bottom-border)スタイルは未対応(今回は全周枠の角丸+消去まで)。
- **✅ FW-15 [fw] P3 🧷 — border 系プリミティブ不足(Container に枠線無し / SecureInput に chrome 無し)= 完了 (2026-06-29, 実機 OK)**
  出所: dogfood チェックリストではなく **Knot UI 完全再現演習**(`examples/knot_clone` + [knot-ui-repro-gaps.md](knot-ui-repro-gaps.md))で炙り出た本命 gap。Tailwind の `border border-gray-300 rounded-lg` が UI 全面に遍在するのに、shroud は枠線を描く手段が乏しかった。確定 gap = G1(`Container` が `background`+`radius` のみで枠線不可)/ G2(`SecureInput` に radius/border/borderless 無し。FW-14 は Input だけ)/ G9(入力欄の常時枠を意図的に付けられない)。ユーザ判断で「Main 写経より先に border を framework へ graduate」を選択。
  対応(framework): **FW-14 の角丸ストローク経路(`stroke_rect_rounded` = `DrawRect.border_width` の SDF)を流用しレンダラ無改造**。① `Container::border(width, color)` 追加 — 塗りの上に重ね描き(透明ボックスも outline 可)、`radius` で角丸、color は `Reactive<Color>` で live theme 追従、`width<=0` は no-border(既定)。② `SecureInput` に `radius`/`border_color`/`borderless` を **Input と対称**に追加。旧式の4矩形 inset border を `fill_rect_rounded`+`stroke_rect_rounded` 1ストロークに置換。focus ring も `radius` 追従化(角丸欄に角丸リング)。**おまけ**: 同じ非対称を抱えてた `Input` の focus ring を `0.0` 固定→`self.radius` に揃え、検証中に見つけた既存の rustdoc gotcha 2件(`scroll_view.rs` / `input.rs` の public doc→private const intra-doc link、[[gotcha-rustdoc-intra-doc-links]])を code span 化(CI doc job `-D warnings` の赤を解消)。**デフォルト不変 ∴ 既存 Container/SecureInput は見た目そのまま**。paint テスト11件(`container_border_*` 6 / `secure_input_*_chrome` 系 5)。`shroud_widgets` 全テスト緑、`cargo clippy --workspace --all-targets -- -D warnings` / `cargo doc -p shroud_widgets`(`-D warnings`)クリーン。
  対応(repro・初使用): [knot_clone unlock.rs](../examples/knot_clone/src/unlock.rs) のパスワード欄に `.radius(8.0)` だけで `rounded-lg border-gray-300` 相当(border は既定で `input_border`=gray300/gray700 追従)。**実機 OK**(2026-06-29 ユーザー確認: 未フォーカスでも常時グレー角丸枠が出る・Tab で角丸 focus ring・Ctrl+D ダークでも枠色自然。※クリックで ring 出ないのは focus-visible ヒューリスティックの意図どおり)。
  - 将来: G3(入力/ボタンの padding・高さ非公開 = `px-4 py-3` 不可)/ G4(非対称 padding)/ G7(focus を外側リング→border 色変化にするか)は別 gap として残置。
- **✅ FW-16 [fw] P3 🧷 — 整列が center 系のみ(G11)/ 片側 border が引けない(G10)= 完了 (2026-07-02, 実機 OK)**
  出所: FW-15 と同じ **Knot UI 完全再現演習**。Main 全 slice で毎回踏んだ2 gap を「回避策はあるが歪む(中)」が繰り返す=本物と判断し、ユーザ選択(FW-16 最小先行)で graduate。確定 gap = G11(`justify-between`/`*-end`/`*-start` 不可 → `grow(1.0)` スペーサで擬似)/ G10(`border-r`/`border-b` 片側線不可 → 1px の兄弟 divider Container で擬似)。
  対応(framework): ① **G11** = shroud ネイティブの `Justify`(Start/Center/End/SpaceBetween/SpaceAround/SpaceEvenly)/ `Align`(Start/Center/End/Stretch)enum を `shroud_layout` に追加 → `FlexStyle::justify/align` + `Container::justify/align`。taffy を widget API に漏らさず `From` で内部マッピング(既存 center 系は温存)。② **G10** = `Container::border_top/right/bottom/left(width, color)` 追加。各辺を sharp `fill_rect` で描画、`Reactive<Color>` で live theme 追従、`width<=0` は無描画。
  **★ 検証中に framework 実バグ発見・同時修正**: 片側 border(や4辺 `border()`)が **full-bleed な子背景に上書きされて消える**。真因 = `paint_node`(tree.rs:1148)が親 `paint()` を**子より先**に描くのに対し `ScrollView::paint` は自矩形**全体**を塗る(scroll_view.rs:298)ので、サイドバー幅いっぱいのノートリストが右 border を隠す(旧 divider は兄弟で全子孫の後=無事だった)。修正 = Container の border 描画を `paint()` → **`paint_post_children()`** に移動(子の後=常に最前面。CSS の border が content box 外に出るのと同じ挙動)。回帰テスト2本(4辺/片側 border が full-bleed 子の上に出る)。**G13/G14 に続く「repro でしか炙れない framework 実バグ」第3号**。
  テスト: layout +5(enum→taffy 全 variant / SpaceBetween・End の実配置)+ widgets +9(各辺の位置・寸法・重なり順・justify 実測・full-bleed 回帰2)。`shroud_widgets` 全テスト緑、fmt / `cargo clippy --workspace --all-targets` / `cargo doc`(`-D warnings`)クリーン。
  対応(repro・実使用): [knot_clone main_screen.rs](../examples/knot_clone/src/main_screen.rs) の回避策を実 API に置換 — サイドバー `border-r` / header・search・title・toolbar の `border-b` / status の `border-t` を `border_*` 化(`divider` fn 削除)、ヘッダ `justify-between`・status `text-right` を `justify(SpaceBetween)`/`justify(End)` 化(ツールバーの `flex-1` は React も本物の spacer なので `grow` 温存)。**実機 OK**(2026-07-02 ユーザー確認: 右端 border 復活・全区切り線と検索枠正常・light/dark 自然)。
  - 将来: G3/G4/G6(padding/寸法非公開)/ G12(Input weight)/ G16(Dropdown 寸法・角、MenuItem ラベル inset)は FW-17 系候補として残置。G5 absolute anchor は `LayerAnchor` の将来変種待ち。
- **✅ FW-17 [fw] P3 🧷 — Input/SecureInput の padding・高さが非公開(G3)= 完了 (2026-07-02, 実機 OK)**
  出所: FW-15/16 と同じ **Knot UI 完全再現演習**。Main slice 1/2/3 と Unlock で**毎回踏んだ最頻 gap**。単一行 Input/SecureInput が `padding(8)` + `min_height(font+20)` を**ハードコード**(`input.rs`/`secure_input.rs`)しており、`borderless()` は枠を消すだけで内部 padding/高さは残る → ① 検索バーが枠付きコンテナに畳むと縦二重 padding で ≈50px(React ≈36)、② 本文の `padding:16px 24px`・タイトルの `text-2xl` 行高・Unlock の `px-4 py-3`≈48px がどれも合わせられない。ユーザ選択(FW-17 最小=寸法 G3 のみ先行、G6 Button/G12 weight/G16 Dropdown は次段)で graduate。
  対応(framework): **デフォルト完全不変**を厳守しつつ Input/SecureInput に3 builder を対称追加。① `padding_x(px)`(左右インセット、既定 8。paint の `text_x`/`max_width`/scrollbar-x + multiline wrap 幅に波及)② `padding_y(px)`(既定 8。単一行は中央寄せゆえ derived `min_height` にのみ効き、multiline は viewport の上下インセット=`text_y`/`viewport_h`/scrollbar track に波及)③ `min_height(px)`(font/行数由来の floor を明示上書き)。style() は `padding(8)` → `padding_trbl(pad_y,pad_x,pad_y,pad_x)`、derived floor は `font+2*pad_y+4`(=既定 pad8 で従来の `font+20`)/ multiline `rows*lh+2*pad_y`(=`+16`)に置換 ∴ 既定値ビット等価。負値は `0.0` にクランプ。
  テスト: widgets +4(default 寸法不変の回帰 / min_height 上書き / padding_y が multiline box を拡大 / padding_x がグリフを inset)+ secure +4(同項目)。**taffy のエッジ丸め**で multiline の差分が subpixel ずれ → 各フィールドを別ツリー y=0 に置いて実測(教訓: 積み上げ配置の寸法 assert は丸めに注意)。`cargo test --workspace` 全緑、fmt / `clippy --workspace --all-targets` / `doc`(`-D warnings`)クリーン。検証中に **knot 本体の既存 rustdoc 赤**(`state.rs:722` の method 間 intra-doc link が `Self::` 無しで未解決、[[gotcha-rustdoc-intra-doc-links]])も潰した(CI doc job drift、[[feedback-prepush-fmt-check]])。
  対応(repro・実使用): [knot_clone](../examples/knot_clone/src) の暫定回避を実 API に置換 — Unlock パスワード欄 `.padding_x(16).min_height(48)`(px-4 py-3)/ 検索バーは input を `padding_x(0).padding_y(0).min_height(20)` にゼロ化し**行の `py-2`(padding 8)に高さを委譲** ≈36px / 本文 `padding_x(24).padding_y(16)` で左 inset スペーサ削除 / タイトル `min_height(32)` で 2rem 行箱。**実機 OK**(2026-07-02 ユーザ確認: Unlock 48px 欄・検索バー締まる・本文 16/24 インセット・タイトル行詰まり、light/dark とも自然)。
  - 将来: G4 は Input/SecureInput 側は `padding_x/y` で非対称化できたが **Container の非対称 padding は未**(`padding_trbl` は内部 API、公開 builder 無し)。G6 Button 寸法 / G12 Input weight / G16 Dropdown・MenuItem は FW-19 系候補として残置。
- **✅ FW-18 [fw] P3 🧷 — drop-shadow / elevation プリミティブが無い(G18 の shadow 半分)= 完了 (2026-07-03, 実機 OK light/dark)**
  出所: FW-15/16/17 と同じ **Knot UI 完全再現演習**の slice 4(Modal 群)で発見。正解モーダルは全て `shadow-xl` で、scrim(`bg-black/50`)の上にカードを**浮かせる**のはこの影 → shroud に shadow/elevation プリミティブが**一切無い**(`DrawRect` = fill+radius+border のみ)ため 4モーダルとも影なしのフラットなカードで、特にダーク背景で境界が溶けていた。演習クローズ後、ユーザ選択(**G18 shadow を graduate**)で着手。
  対応(framework): **新パイプライン無し**で既存の角丸 SDF rect を拡張。① `DrawRect.blur` を1本追加、`build_rect_geometry` が `blur>0` でクアッドを blur 分だけ外側に膨らませ、rect シェーダが `1 - smoothstep(0, blur, sdf_rounded_rect(...))` でフェード(枠内は不透明・外へ `blur`px で 0)。**`blur==0` は膨張 0・従来分岐へ短絡 ∴ 既存 fill/border とビット等価**。② `PaintContext::fill_shadow(rect, color, radius, blur)`(offset/clip を他 draw 同様に畳み込み、`blur<=0` は no-op)。③ `Container::shadow(offset_x, offset_y, blur, spread, color)`(CSS `box-shadow` 準拠。背景の**手前ではなく背後**=`paint()` 冒頭で描画 → 不透明背景が内側を覆い、はみ出た halo だけ見える。spread は箱を膨縮 + radius を追従、負 spread で halo を絞る)+ `Container::elevation(1..=4)` preset(1 resting / 4 = modal `shadow-xl`)。
  テスト: paint +3(offset 畳み込み+blur 記録 / `blur<=0` no-op / fill_rect が blur=0 維持)+ widgets +7(shadow が fill の背後 / 既定は無影 / offset+spread が caster box に畳まれる / radius が spread 追従 / 透明色は無描画 / elevation preset が影を出す / elevation(0) 無影)。**shroud_widgets 277 test 緑**、`fmt --all --check` / `clippy(shroud_render/widgets/knot_clone --all-targets)` / `doc`(`-D warnings`)クリーン([[feedback-prepush-fmt-check]])。
  対応(repro・実使用): [knot_clone modals.rs](../examples/knot_clone/src/modals.rs) の `card_column`(全4モーダル共通シェル)に `.elevation(4)` を1行追加 → Backup/ChangePw/Restore/Confirm 全部が `shadow-xl` 相当に。**実機 OK**(2026-07-03 ユーザ確認、light/dark 両方のスクショでカードが scrim から浮くのを確認)。
  - 残置: **G18 のもう半分 = viewport 相対サイズ(vh/vw)** は layout 側の別作業(layout に viewport を渡す口が要る)ゆえ今回スコープ外。Restore は `max-h-[80vh]` → `max_height(576)`(720px の 80%)ハードコードのまま。FW-19 系候補: G6 Button 寸法 / G12 Input weight / G16 Dropdown・MenuItem / G4 Container 非対称 padding / vh-vw。
- **✅ FW-19(最小)[fw] P3 🧷 — Button 寸法/disabled(G6)+ Container 非対称 padding(G4)+ Input font-weight & reactive chrome(G12)= 完了 (2026-07-03, 実機 OK light)**
  出所: FW-15/16/17/18 と同じ **Knot UI 完全再現演習**で出揃った残 gap のうち「複数 slice で繰り返し踏んだ・app author が自然に踏む」表現力 gap 3点。演習クローズ後、ユーザ選択(**FW-19 最小 = G6/G4/G12 のみ先行**、G16 polish / G5 absolute anchor / vh-vw は次段)で graduate。
  対応(framework):
  - **G4** — `Container::padding_xy(x, y)`(Tailwind `px-* py-*`)+ `Container::padding_trbl(t,r,b,l)`(全辺独立)を公開。内部 `FlexStyle::padding_trbl` の薄いラッパ、負値クランプ。
  - **G6** — `Button` に `padding_x/y`(既定 8 でビット等価、ハードコード `padding(8)` を置換)/ `min_width`(**measured-leaf 不変条件**を守り style `min_size` でなく `measure` 側で content を `min_width - 2*pad_x` に floor)/ `disabled(Reactive<bool>)`(hover/press フェード停止・`focusable()=false`で Tab 除外・`on_click` 不発)/ `disabled_background`(既定は通常背景+ラベルをアルファ半減、テーマ非依存の greyed-out)。
  - **G12** — `Input::weight(FontWeight)`。**caret/hit-test/選択の幾何も同じ weight で整形**するのが要点(色だけの highlight と違い bold は advance が変わる)→ text engine に attrs 版を追加(`offset_at_point_attrs` / `selection_rects_attrs`(+`_with_trailing_attrs`) / `caret_at_offset_attrs`。既存メソッドは `TextAttrs::default()` へ委譲=挙動不変)。併せて Input の chrome setter(`background`/`border_color`/`text_color`/`focus_ring_color`/`selection_color`)を `Color`→`impl Into<Reactive<Color>>` 化(live テーマ追従、`Color` は `Into` で従来呼び出しと source 互換 ∴ knot 無改変)。`build_highlight_spans` も base attrs を各 span に載せ、weight+highlighter 併用時も幾何一致。
  テスト: shroud_text +3(bold の caret/選択が **bold 整形幅**に一致 / default-attrs 委譲が既存 `offset_at_point` と一致)+ shroud_widgets 新規 [fw19_tests.rs](../crates/shroud_widgets/tests/fw19_tests.rs) 10本(padding_xy/trbl の軸別 inset・負値クランプ / padding_y が箱高を 2×Δ 伸ばす / min_width floor / disabled が click 不発・reactive gate・背景アルファ半減 / Input 背景が signal 追従・reactive setter 受理)。**shroud_widgets 277 + text 全 test 緑**、`fmt --all --check` / `clippy(--workspace --exclude knot --all-targets -D warnings)` / `doc(-D warnings)` クリーン([[feedback-prepush-fmt-check]])。
  対応(repro・実使用): [knot_clone main_screen.rs](../examples/knot_clone/src/main_screen.rs) のエディタ**タイトル**を実 `weight(BOLD)`(G12)化し、タイトル**節**を真の `padding_xy(24, 16)`(=`px-6 py-4`, G4)へ格上げ。従来は uniform `padding(16)` + 各子の 8px inset で `px-6` を近似していた補正(Saved/tags の先頭スペーサ)を撤去 → 既知の**アクション群の右端 8px ズレも解消**。**実機 OK**(2026-07-03 ユーザ確認 light: タイトル太字・px-6 左揃い・アクション群右端が締まるのを確認。dark 未確認だが同 idiom、[[feedback-screenshot-color-hdr]])。
  - 残置(FW-19 の続き候補): G16 Dropdown 寸法/角丸・MenuItem 左寄せ / G5 absolute・固定エッジ anchor・右寄せ placement・click 時 trigger rect / G18 の vh-vw / G3 残(固定高 multiline viewport)/ G7 focus 方式 / G15 hover 固定。SecureInput の weight は無意味(mask)ゆえ非対象、chrome-Reactive も Input のみ先行。
- **✅ FW-20 [fw] P3 🧷 — Dropdown 寸法/角丸 + MenuItem ラベル左寄せ(G16 polish)= 完了 (2026-07-04, gate 全緑・実機確認待ち)**
  出所: FW-15〜19 と同じ **Knot UI 完全再現演習**。slice 3(overlays)で設定 dropdown を実機確認したユーザ指摘の「惜しい」3点(忠実再現は妨げないが質感差)。FW-19 の続き(polish)として graduate。
  対応(framework):
  - **Dropdown が React `<select>` より大きい** — `measure` が content 高を `font+16` で返し **taffy がさらに vertical padding(2×8)を足す二重計上**で border box = `font+32`(≈48px。React `px-2 py-1 text-sm` は ≈28px)。`padding_x`/`padding_y`/`min_height` builder を Input/Button と対称に公開し、measure の高さを **content = 1 行(`line_height`)/ 箱高 = `line_height + 2*padding_y`** に是正(doubling 解消)。`min_height` は border-box floor(padding を引いて content floor に変換)。既定は `padding_x=12`/`padding_y=8` で、既存テストの下限(`>= font+16`)を満たしつつ default も ≈font+16 に締まる(旧 font+32 から縮小、消費者は clone のみ)。
  - **Dropdown の角が四角い** — トリガ枠を sharp `fill_rect`×4 → `stroke_rect_rounded` 1本に置換し `radius` 追従(FW-15 で角丸化した Input/SecureInput と対称)。DropdownPopover の枠も同様。
  - **MenuItem のラベルが箱左端に張り付く** — `paint` が `text_x = origin.x` で描き、宣言済み `padding_trbl(_,12,_,12)` の左 inset を無視していた。padding を `H_PADDING`/`V_PADDING` 定数に切り出し style と paint で共有、`text_x = origin.x + H_PADDING`・shape 幅も `width - 2*H_PADDING` に。context menu / actions / 設定パネルのラベルが `px-3` 相当で inset される。
  テスト: dropdown_tests +4(default 高が font+32 に戻らず 1 行+padding / `padding_y` で箱高可変 / `min_height` が border-box floor / 枠が **1本の rounded stroke**)+ context_menu_tests +1(ラベルが ~12px inset)。**shroud_widgets 全 test 緑**、`fmt --all --check` / `clippy(shroud_widgets+knot_clone --all-targets -D warnings)` / `doc(-D warnings)` クリーン([[feedback-prepush-fmt-check]])。
  対応(repro・実使用): [knot_clone main_screen.rs](../examples/knot_clone/src/main_screen.rs) の `select()` に `padding_x(8).padding_y(4)`(`px-2 py-1`)を付け ≈25px に締め、設定行(`settings_select_row`/`settings_sort_row`)を `padding_xy(12, 8)` にして MenuItem の 12px inset と横位置を揃えた。
  - **★ 実機レビューで判明したクローン fidelity バグ(framework ではない)**: MenuItem 修正後、設定メニューの「Change Master Password」が2行に折り返すのをユーザが指摘。当初「React も折り返す=忠実」と誤答したが、**React 実物のラベルは `t()` i18n で "Change Password" / "Auto-backup" / "Import" / "Export all notes" / "Restore welcome note"** であり、クローンが**勝手に長い・大文字寄りの文言("Change Master Password"/"Backup Settings"/"Import..."/"Export All"/"Restore Welcome Note")を置いていた**のが真因。[translations.ts](../knot-notes-app-v0.7.0-2026-04-27/src/i18n/translations.ts) の EN 値で照合し、設定/アクション両メニュー + モーダル見出し + context menu の "Export…"→"Export" まで実文字列に修正。短い実文字列なら w-48(168px 相当)に一行で収まり、MenuItem の wrap+grow 自体は正しい挙動(=framework 無罪)。**教訓: 「折り返しが React と一致」を主張する前に、クローンのラベルが実 i18n 文字列と一致しているか先に確認する。**
  **gate 全緑(clone は check/clippy/fmt クリーン)。実機は knot_clone.exe が起動中でロック → ユーザが閉じて再ビルドで確認予定。**
  - 残置(FW-20 後の続き候補): G5 absolute・固定エッジ anchor・右寄せ placement・click 時 trigger rect / G18 の vh-vw / G3 残(固定高 multiline viewport)/ G7 focus 方式 / G15 hover 固定。
- **✅ FW-21 [fw] P3 🧷 — absolute/固定エッジ anchor + 右寄せ placement + click 時 trigger rect(G5 の3点)= 完了 (2026-07-04, commit `0aba208`, 実機 OK light)**
  出所: FW-15〜20 と同じ **Knot UI 完全再現演習**。slice 3(overlays)で「Layer で absolute をどこまで代替できるか」を検証した際に炙り出た **G5 の3つの open 点**(バナー top-center / メニュー右寄せ / ボタン rect 取得)を、演習クローズ後にまとめて graduate。ユーザ選択(FW-20 後に「G5 を終わらせる」)。
  対応(framework):
  - **① click 時に trigger の rect が取れない** — `Container::on_press_rect(FnMut(Rect, &mut EventContext))` を追加(`on_hover_enter` の press 版)。`MouseDown` で widget 自身の `layout` rect を渡し consume。`on_press`(点)と別フィールドで併用可・両方 fire。layer 内では rect は layer-local だが、`push_layer` の既存 offset 変換(G14)で `AnchorRect` がそのまま viewport 化されるので nested でも正しく落ちる。
  - **② 右寄せ placement が無い** — `LayerAnchor::AnchorRect` に `align: HAlign` を追加。`HAlign::{Start,Center,End}` = popover の左辺/中央/右辺を trigger の対応辺に合わせる(End = CSS `right-0`、左に開く)。`place_layer` の x 計算を align 分岐化(既存は Start = 従来挙動)。
  - **③ 固定エッジ / オフセット anchor が無い** — 新変種 `LayerAnchor::Viewport { h: HAlign, v: VAlign, offset: (f32,f32) }`。viewport の角/辺 + ピクセル nudge で CSS `absolute`/`fixed` 相当。両軸クランプ。viewport 絶対ゆえ `push_layer` は変換しない(`ViewportCenter` と同挙動)。`ViewportCenter` は `Viewport{Center,Center,(0,0)}` の名前付きショートハンドとして温存。`HAlign`/`VAlign` を `shroud_widgets::layer` に追加し `pub use` 再エクスポート。
  - **破壊的変更(小)**: `AnchorRect` にフィールド追加のため全構築サイト(dropdown・event/layer/tooltip tests・**knot sidebar/tooltip**・knot_spike)に `align: HAlign::Start` を機械的追記(挙動不変)。knot はこの1点のみ変更 = enum が育った正当な source break([[feedback-roadmap-hygiene]]・過去の on_frame 変更と同格)。
  テスト: layer_tests +4(End=右寄せ / Center / Viewport top-center+offset / Viewport bottom-right 負 offset)・event(lib) +2(align が offset 変換を跨いで保持 / Viewport 非変換)・press_blur_tests +1(on_press_rect が cursor 点でなく box rect を返す)。**shroud_widgets 全 test 緑**(layer 36 / lib 63 / press_blur 6 / widget 277 …)、`fmt --all` / `clippy(--workspace --exclude knot -D warnings + knot -D warnings)` / `doc(-D warnings)` クリーン([[feedback-prepush-fmt-check]])。
  対応(repro・実使用): [knot_clone main_screen.rs](../examples/knot_clone/src/main_screen.rs) — gear/⋮ ヘッダメニューを `menu_icon_trigger` の `on_press` → `on_press_rect` + 新ヘルパ `open_popover_below_rect`(`AnchorRect{prefer:Below, align:End}`)にして React の `right-0 top-full`(ボタンに anchor・右寄せ・下開き)を 1:1 に。context menu は cursor anchor のまま(正しい)。エラーバナーを `Viewport{Center, Start, offset:(128,8)}` に(x=128 = サイドバー 256px の半分 → エディタペイン中央 = React の pane-relative `left-1/2` と一致、y=8 = `top-2`)。
  **gate 全緑。実機 OK(light、2026-07-04 ユーザ確認 — 設定メニューが gear に anchor・エラーバナーがエディタペイン上端中央)。HDR ∴ 色はユーザの目が正([[feedback-screenshot-color-hdr]])。**
  - 残置(FW-21 後の続き候補): G18 の vh-vw / G3 残(固定高 multiline viewport)/ G7 focus 方式 / G15 hover 固定。**演習で炙った表現力 gap は G7/G15(共に設計判断・棚上げ)を除き graduate 完了。**

#### Knot app — UX 磨き

- **AP-1 [app] P2 — 複製ボタンが背景と同化して見えない** (元 #42) — context menu / row のコントラスト
- **AP-2 [app] P2 — ゴミ箱ビューで「+新規」等が押せて混乱** (元 #43) — trash view ではアクション無効化/非表示
- **AP-3 [app] P2 — backup 既定パスが AppData(隠し)で分かりにくい** (元 #44) — Documents 等に変更 or 初回に明示
- **AP-4 [app] P2 — 検索のスペースは AND に** (元 #41) — token 分割を AND セマンティクスへ
- **AP-5 [app] P2 — ライトモード配色がまぶしい** (元 #32) — light theme の surface/bg 輝度調整
- **AP-6 [app] P2 — wikilink 挿入ボタン (toolbar)** (元 #37)
- **AP-7 [app] P2 — backlinks パネルを折りたためるように** (元 #38)
- **AP-8 [app] P3 — wikilink hover で見た目変化** (元 #39)
- **AP-9 [app] P3 — タグ表示行の縦を削る** (元 #36)
- **AP-10 [app] P3 — 削除後の選択挙動 / empty-state 画面** (元 #30)
- **AP-11 [app] P3 — ソート昇順/降順トグル** (元 #33)

### → 様子見 / 仕様(まだ動かさない)

- **元 #23 パスワード強度メーター** — [[roadmap]] で「強度ポリシーは*意図的に非移植*」と記録済。今回は据え置き。E(リリース)前に復活させるか改めて判断。
- **元 #35 live split preview が使いにくい** — **要設計判断**。実装ではなく方向決めが先: ①split の比率/プレビュー見やすさを改善 ②toggle 方式(編集⇄プレビュー)に戻す ③可変ペイン幅。決めてから昇格。
- **元 #31 フォントに差があって見づらい** — **要再現**。どの画面のどのフォント差か曖昧。次の dogfood で具体化してから。

### → 棄却

(なし — 全件 legit だった)

---

## 使い込みチェックリスト(機能を一通り実地で踏む)

普段使いで自然に触る順。踏んで問題なければ ✅、引っかかったら Inbox 行き。

- [x] 初回 setup(master pw 設定 2回 Enter)+ recovery key を実際に PDF 保存
- [x] lock → unlock を1日数回(auto-lock も含めて自然に)
- [ ] 長いノート(数千字)を編集 — viewport スクロール・scroll-to-caret の体感
- [x] markdown 編集中の smart keymap(list 継続・空行 exit・quote)
- [ ] live split preview を常用してみる
- [x] find/replace(Ctrl+H)を実タスクで使う
- [ ] tag を実際に運用(付ける・autocomplete・sidebar filter で絞る)
- [x] wikilink `[[...]]` でノート間を行き来 + backlinks panel
- [ ] 画像を D&D / クリップボード貼り付けで実挿入
- [x] full-text search(Ctrl+F)で過去ノートを探す
- [x] note 複製(右クリック)・trash → restore → 完全削除
- [x] .md import / export を実データで
- [x] theme / font-size 切替 + 再起動して設定が残るか
- [x] backup(.knotbak)→ 別の場所で restore できるか
- [x] ja ⇄ en 切替で表示が破綻しないか
