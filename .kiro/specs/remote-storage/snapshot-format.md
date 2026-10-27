# Snapshot row format v1

2.4は共有codec、CRMのtable/column/key許可表、CRMのsequence検査を所有する。controlの許可表はappの4.1で所有し、認証schemaをcoreへ複製しない。実remoteのページ読取りは2.5、サービス全体の保存・復旧は4.1/4.2で検証する。

## 行の表現

各行は改行を除いて1MiB以下のUTF-8 JSONとし、次の形だけを受け付ける。

`{"version":1,"values":[{"type":"integer","value":"42"},{"type":"null"}]}`

valuesは許可表で固定した列順で、列数も一致させる。未知field、未知tag、重複field、非canonical表現、版違いを拒否する。encoderが生成する空白なしのJSONへの再encode一致でcanonicalを確認できる。保存物からtable名・列名・SQLを生成しない。

| type | value |
|---|---|
| null | fieldを持たない |
| integer | i64範囲のcanonicalな10進文字列。f64を経由しない |
| real | 有限f64のIEEE754 bitを小文字16桁hexで表す。負の0を保つ |
| text | UTF-8文字列 |
| blob | padding付きstandard Base64のcanonical文字列 |

encoder/decoderは1行ずつ扱い、過大入力を構造化前に拒否する。型と長さの計算はoverflowを拒否し、encoded行とdecode作業領域を制限する。ページ全体をencoded行のVecへためない。

## 許可表とsequence

CRMの対象tableはdesignのData Modelsに従い、各列順とkeyset順を既存DDLから固定する。生成列has_phone、FTS shadow、erase_gateは行として保存しない。erase_gateの空確認は読取り側で行う。controlのsessions/gateも保存対象外である。

既知AUTOINCREMENT表はcustom_field_definitionsだけである。目録内は`"sequences":{"custom_field_definitions":"42"}`のようにcanonicalな10進文字列で記録する。形式v1はそのhigh-waterを必ず明記し、未知表、重複、欠落、負値、i64 overflow、現存最大id未満を拒否する。

SQL sourceにsequence行がなく、custom_field_definitionsが空なら0として明記できる。現存idがあるのにsequence行がなければ拒否する。保存物のsequence項目欠落を0で補わない。削除後も残るhigh-waterを保持し、実際の復旧後に最大IDが再利用されない検査は4.2で行う。

metaのDB識別・版・restoring/ready遷移はlifecycle側が扱う。codecはその値を解釈して保存先を変更せず、未知schemaやSQLを実行しない。moduleと追加依存はcloud限定とする。
