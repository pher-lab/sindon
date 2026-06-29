# Knot UI 完全再現 — gap ログ

**目的**: React/Tailwind 版 Knot (`knot-notes-app-v0.7.0-2026-04-27`) を「正解画面」とし、
shroud で見た目を写経する。写し取れない / 不格好になる箇所を gap として収集する。
機能 dogfood (FW-1〜14) が「動くか」を見てきたのに対し、これは **表現力 (見た目) の天井** を炙る。

- 再現先: `examples/knot_clone`（UI-only。crypto/db なし、ダミーデータ）
- 正解参照: React ソース + Tailwind パレット（ビルドせず静的に spec 化）
- 確定した gap は後で FW-15+ として `docs/dogfood-log.md` に graduate する

凡例: **重大** = 忠実再現を妨げる / **中** = 回避策はあるが歪む / **小** = 些細

---

## 画面別 進捗

| 画面 | 状態 |
|------|------|
| Unlock | 完了（gap 由来の差を除き React 版と一致） |
| Setup | 未 |
| Recovery | 未 |
| Main — Sidebar shell | 進行中（slice 1: パネル/ヘッダ/New Note/検索/ノート行） |
| Main — Editor pane | 未（slice 2） |
| Main — Overlays (設定 dropdown / エラーバナー / context menu) | 未（slice 3） |
| 各種 Modal (Settings/Backup/ChangePw/Restore) | 未 |
| Loading | 未 |

---

## Gap 一覧

### G1. Container に border がない → **解消（FW-15、2026-06-29 landed）**
- 正解では border が遍在する: input (`border border-gray-300`)、エラー枠 (`border-red-300`)、
  カード、テーブルセル、サイドバー区切り、select。
- ~~shroud の `Container` は `background` + `radius` のみで枠線を描けない。~~
- 対応: `Container::border(width, color)` を追加。FW-14 の Input border と同じ
  `stroke_rect_rounded` SDF 基盤を流用。`radius` で角丸し、塗りの上に重ねて描く
  ので透明ボックスも outline だけ持てる。color は `Reactive<Color>` なので live theme
  追従。`width <= 0.0` は no-border（既定）。テスト6本 (`container_border_*`)。

### G2. `SecureInput` に chrome カスタマイズがない（radius/border/borderless）→ **解消（FW-15、2026-06-29 landed）**
- ~~FW-14 で `Input` には radius/border_color/borderless が載ったが SecureInput は未反映。~~
- 対応: FW-14 の chrome を `SecureInput` に対称展開 — `radius` / `border_color` /
  `borderless` を追加。旧式の4矩形 inset border を Input と同じ
  `fill_rect_rounded` + `stroke_rect_rounded` 1ストロークに置換。focus ring も
  `radius` 追従にした（角丸欄に角丸リング）。**おまけ**: 同じ非対称を抱えていた
  `Input` の focus ring も `0.0` 固定 → `self.radius` に揃えた。テスト5本
  (`secure_input_*_chrome` 系)。Unlock のパスワード欄は `.radius(8.0)` だけで
  `rounded-lg border-gray-300` に（border は既定で `input_border` 追従）。

### G3. Input / SecureInput の padding・高さが非公開 — **中**
- 正解の入力欄は `px-4 py-3`（≒ 高さ 48px）。shroud の Input/SecureInput は内部 padding 固定で
  寸法を合わせられない。ボタンも同様（`py-3`）。
- 実装裏付け（`input.rs:1469`）: 単一行 Input は `FlexStyle::new().padding(8.0)
  .min_height(font_size + 20.0)` を**ハードコード**。`padding(8)` も `min_height`（font 14 →
  34px）も非公開。
- **実害例（Main slice 1 — 検索バーが「妙に縦長」）**: React は「枠付き div（`px-3 py-2`）+
  bare な borderless input」で ≈36px。shroud で同じ構成にすると「枠付きコンテナ padding 8 +
  borderless Input（自前 padding 8 + min_height 34）」= **≈50px** に膨らむ。`borderless()` は
  *枠*を消すだけで*内部 padding と min_height は残る*ため、カスタム chrome 行に Input を
  畳み込むと縦 padding が二重になる。
  - しかもこの1要素が G3 と **G4 を同時に踏む**: uniform padding なので「縦を詰める（コンテナ
    padding を下げる）」と「🔍 アイコンの左インセット」がトレードオフになり両立しない。
  - clone の暫定: コンテナ padding を 8→4 に下げて高さを ≈42px に寄せた（完全一致は不可）。
- 候補対応: Input/SecureInput に `padding`(x/y) もしくは `min_height` を公開、または
  `borderless()` 時に内部 padding/min_height も落とす「chrome 無し」モード。Button も同様。

### G4. `padding` が上下左右一律のみ — **中**
- Tailwind は `px-4 py-3` / `px-2 py-1` のような非対称 padding が常用。
- `Container::padding(px)` は一律のみ。`padding_xy(x, y)` / 各辺 padding が欲しい。

### G5. absolute / コーナー配置のプリミティブがない — **中**
- Unlock 右上の言語 select は `absolute top-4 right-4`。通常フローの外に置く手段が
  Container 系にない（`Layer` + anchor で代替可能か要検証）。
- 第一稿では言語 select を省略。

### G6. Button の padding / 高さ / disabled スタイルが非公開 — **中**
- 正解の submit は `w-full py-3` かつ `disabled:bg-blue-800 disabled:cursor-not-allowed`。
- Button は `radius`/`background`/`text_color`/`hover_background`/`press_background` はあるが
  padding・高さ・disabled 状態スタイルがない。w-full は flex stretch で代替予定（要確認）。

### G7. focus モデルの差（外側リング vs border 色変化）— **小〜中（要判断）**
- 正解は `focus:outline-none focus:border-blue-500`＝**枠線の色が変わるだけ**。
- shroud は外側に focus ring を描く（offset 付き）。見た目の質感が異なる。
- 「どちらが正か」は設計判断。Knot 再現の観点では border-color 方式が欲しい場面がある。

### G8. ~~sRGB hex で色が洗い出される（ガンマ非対応）~~ → **誤検知（取り下げ）**
- 第一稿のスクショで全色が洗い出されて見えたが、**原因は描画ではなく HDR→SDR スクショ側の
  アーティファクト**。ユーザの実機（HDR 環境）では `from_rgba8` がそのまま正しく描画されており、
  HDR を切ったら素の `from_rgba8` で自然な色になった。
- 一度入れた `tokens::s2l()`（sRGB→linear デコード）は**過補正**なので撤去済み。
- 結論: **shroud に色空間バグは無い**（少なくとも本件では）。`from_rgba8` で web hex はそのまま出る。
- ⚠ **方法論メモ**: この PC では**スクショの色は当てにならない**（HDR 起因）。以後スクショは
  **レイアウト/構造判定専用**にし、**色の忠実度はユーザの目を正**とする。framework の色バグを
  スクショだけで断定しない。

### G9. 入力欄の枠が弱い（未フォーカスでほぼ枠なし）→ **解消（G1/G2 の派生）**
- ~~未フォーカスで枠がごく薄く、border/radius ビルダーが無いので常時枠を意図的に付けられない。~~
- G2 で `SecureInput` / `Input` とも border が既定 ON（`input_border` 追従）+ `radius`
  対応になったので、未フォーカスでも常時 `border border-gray-300 rounded-lg` 相当が出る。
- 残る差は G7（フォーカス時に「外側リング」が出る vs 正解は「枠線の色が変わるだけ」）。
  これは設計判断としてまだ open。常時枠が出るようになった分、G7 の体感差は小さくなった。

### G10. 片側 border (`border-r` / `border-b`) が引けない — **中**
- 正解は仕切り線を片側 border で多用する: サイドバー右端 `border-r`、ヘッダ／検索／タグ各
  セクション下端 `border-b`、dropdown 内の区切り。
- shroud の `Container::border(width, color)`（FW-15）は**4辺一括のみ**。1辺だけの線が引けない。
- 暫定対応（Main slice 1）: 1px の細い `Container`（`height(1).width_full()` または
  `width(1).height_full()` に border 色を `background`）を仕切り線として挟む。動くが、
  「box の辺」ではなく「兄弟ノード」なので角の処理や padding 内側への食い込みは別物。
- 候補対応: `border_bottom(w,c)` / `border_right(w,c)` ないし `border_sides(...)`、
  もしくは `Divider` プリミティブ。

### G11. flex 整列が `center` 系しかない（`justify-between` / `*-end` / `*-start` 不可）— **中**
- 正解のヘッダ行は `flex items-center justify-between`（タイトル左・操作ボタン群右）。行・
  列の両端寄せ・端寄せが頻出（モーダルのフッタボタン、リスト行の右端メタ等）。
- shroud は `center` / `justify_center`（主軸中央）/ `align_center`（交差軸中央）のみ。
  `justify-between` / `justify-end` / `align-start` / `align-end` に当たる builder が無い。
- 暫定対応（slice 1）: 両端の間に `Container::row().grow(1.0)` のスペーサを挟んで
  `justify-between` を擬似再現。`justify-end` も先頭スペーサで代替可だが冗長。
- 候補対応: `justify(Justify)` / `align(Align)` で `Start/Center/End/Between/Around` を公開。

### アイコンについて（gap ではない・記録のみ）
- 正解は inline SVG アイコン。clone はアイコンフォント未同梱なので、ヘッダ操作・検索・ピン留め
  などは単一グリフ（⚙ ⋮ 🗑 🔒 🔍 📌）で近似した。アイコン描画の正式手段は FW-12（`App::font`
  + `family(Named)`）で既に解決済みであり、本 UI-only clone の対象外。レイアウト/枠/余白の
  gap 判定に影響しないため近似で進める。

<!-- 以降、画面を進めながら追記 -->
