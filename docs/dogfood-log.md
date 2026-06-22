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
- **FW-7b [fw] P2 — スムーススクロール (Input 内部 viewport)** — FW-7 から分離。multiline Input の内部 `scroll_y: Cell` は **wheel(smooth 化したい)と scroll-to-caret(必ず即時)が同じ offset を共有**しており、paint-authoritative clamp とも絡む(B-1⑤ editor viewport)。`Animated::snap`(= scroll-to-caret 用)は用意済なので、wheel だけ `set`・caret reveal は `snap` で載せられる見込みだが、回帰リスクがあるので別 slice。未着手。
- **FW-8 [fw] P3 — focus ring がクリックでも出る(:focus-visible 相当が無い)**
  真因: [input.rs](../crates/shroud_widgets/src/input.rs) は `if self.focused { ctx.paint_focus_ring(..) }`(1859 付近)で focus 状態だけ見て ring を描き、focus の**理由(pointer / keyboard)を区別しない**。auto-focus-on-click でクリックでも focused → ring 表示。対応: FocusManager に focus reason を持たせ、pointer 起因の focus では ring を抑制(keyboard/Tab のみ表示)。Button/Checkbox/Dropdown も同経路。
- **FW-9 [fw] P3 — focus ring が四角で角丸でない**
  真因: [paint.rs](../crates/shroud_widgets/src/paint.rs) `paint_focus_ring`(277-290)は 4 本の sharp `fill_rect` で枠を描く。`fill_rect_rounded`(193, SDF 角丸)は既にあるので、widget の radius を `paint_focus_ring` に渡して角丸 stroke 化すれば解消。**FW-8 と同じ focus-ring paint 周りなのでまとめて着手が効率的**。
- **FW-10 [fw] P3 — triple-click で行選択**
  現状: double-click 単語選択は実装済(`word_bounds`/`DOUBLE_CLICK_MAX`)だが、`last_click` は double 発火後に reset され **triple は連鎖しない**設計(input.rs 338 付近)。対応: click count を追い、triple = 視覚行/段落の選択に拡張。**FW-11 の現実的 fallback も兼ねる**。

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

- **FW-11 [fw] P3 — 日本語等の単語選択(double-click)が機能しない** — **要設計判断**。真因確定: [input.rs](../crates/shroud_widgets/src/input.rs) `classify`(237)が `is_alphanumeric()` を使うため CJK(漢字/ひらがな/カタカナ)を全部 `Word` 扱い → double-click が **CJK の連続全体**(実質その文/行まるごと)を掴む。正しい分かち書きには辞書(MeCab 級)が必須で、zeroize-first の最小 framework に積むには重い。方向案を決めてから昇格: ①辞書なしで script-run(Han/Hiragana/Katakana/Latin の切れ目)区切りにして「多少マシ」にする ②そもそも word-select は諦め、**triple-click 行選択(FW-10)を実用的 fallback とする** ③現状維持(連続全選択)で割り切る。ユーザー文言「使えないなら使えないで何かできないか」= ①か②を期待。
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
