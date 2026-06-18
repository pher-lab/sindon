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

(2026-06-18 の初回バッチ22件はトリアージ済みへ移動)

---

## トリアージ済み

> 2026-06-18 初回トリアージ。`[fw]` = shroud framework 側 / `[app]` = Knot 側。
> framework 項目が**リリース対象本体の穴**なので最優先。P1 2件 (FW-1/FW-2) は実コードで真因確定済み。

### → 昇格(roadmap / 実装行き)

#### framework (shroud) — リリース前に効く本丸

- **FW-1 [fw] P1 — IME 未確定文字列(preedit)が表示されない** (元 #25, #24)
  真因確定: [event_loop.rs:1071](../crates/shroud_app/src/event_loop.rs) が `Ime::Preedit` を明示的に無視 (`Commit` のみ処理)。winit は `set_ime_allowed(true)` 時にインライン変換文字列を OS に描かせず **アプリに描画させる** 設計なので、無視 = 確定まで何も見えない。コメントの「OS が変換窓を出す」前提は誤り。
  対応: focused Input に preedit state を持たせ、caret 位置に下線付きで描画 (commit で破棄)。**#24 (focus で IME open 強制 → ひらがな default) も同じ IME 配線** ([event_loop.rs:916-930](../crates/shroud_app/src/event_loop.rs) の `ImmSetOpenStatus(true)` 強制) なので束ねて見る。
- **FW-2 [fw] P1 — soft-wrap 折り返し後の ↑↓ 移動が壊れる** (元 #26, #27, #29)
  真因確定: [input.rs:1313](../crates/shroud_widgets/src/input.rs) のとおり ↑↓ が **hard line(段落)単位**で、視覚的な折り返し行を見ていない → 折り返し段落内で「見当違い」にワープ。sticky column も x 座標でなく文字 index 基準 (#27 の「文字サイズ未考慮」)。
  対応: caret ↔ 視覚行(x,y) マッピングを text engine の hit-test 越しに。**#29 (端で行頭/行末へ吸着) も同じ改修に同梱**。
- **FW-3 [fw] P2 — 右端 caret がスクロールバーに被る** (元 #34)
  [input.rs:1667 周辺](../crates/shroud_widgets/src/input.rs) の wrap_width / scrollbar gutter の見直し。FW-2 と同じ viewport 領域。
- **FW-4 [fw] P2 — color emoji が真っ白** (元 #28)
  text engine が monochrome alpha glyph のみ。COLR/bitmap emoji 非対応。**大きめ**(atlas に color glyph 経路追加)・優先度中。
- **FW-5 [fw] P2 — 画像が荒い** (元 #40)
  image pipeline のサンプリング/フィルタ要調査(linear / mipmap / DPI スケール)。要再現で原因切り分けてから。

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
