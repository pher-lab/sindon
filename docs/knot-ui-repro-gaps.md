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
| Main (Sidebar + Editor) | 未 |
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
- 候補対応: 入力・ボタンに `padding`(x/y) もしくは `min_height` を公開。

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

<!-- 以降、画面を進めながら追記 -->
