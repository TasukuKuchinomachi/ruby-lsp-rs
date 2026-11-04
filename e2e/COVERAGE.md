# Steep 2.1.0 の検査項目と end-to-end ケースの対応

`steep-check` が Steep 2.1.0 と同じ結果を出すことを、Steep の smoke test（`smoke/` 以下）と、このディレクトリの `cases/` 以下のケースの両方を `steep check` で走らせて確かめます。この文書は、Steep が検査する項目を Steep のソースから洗い出し、それぞれがどちらで確かめられているかをまとめたものです。

## ケースの作り方と確かめ方

- 各ケースは Steep の smoke test と同じ形をしています。`Steepfile`、Ruby のファイル、RBS のファイル、`test_expectations.yml` を 1 つのディレクトリに置きます。smoke test と同じく、RBS は基本的に `*.rbs` としてケースの直下に置きます。`Steepfile` の `check` や `signature` にディレクトリを渡すことを確かめるケース（`target_group` など）だけは `lib/` と `sig/` を使います。
- `test_expectations.yml` は手で書かず、Ruby 3.4.11、steep 2.1.0、rbs 4.2.0 で、Steep の `bin/output_rebaseline.rb` と同じく各ケースのディレクトリで次を実行して作りました。

  ```sh
  mise exec ruby@3.4.11 -- steep check --save-expectations=test_expectations.yml
  ```

- すべてのケースが、Steep の `bin/output_test.rb` と同じ次のコマンドで成功することを確かめています。

  ```sh
  mise exec ruby@3.4.11 -- steep check --with-expectations=test_expectations.yml
  ```

- 期待値は `--severity-level` を指定しない（既定の `warning`）状態で作っています。smoke test と同じく、多くのケースは `configure_code_diagnostics(Steep::Diagnostic::Ruby.all_error)` ですべての診断を error にしています。
- 1 つのケースでは 1 つの項目を確かめます。正しいコードで診断が出ないことを確かめるだけのケースは `argument_forwarding` と `diagnostic_configuration_silent` だけで、ほかは診断が出るケースです。型を見せるために、存在しないメソッド `no_such_method` を呼んで `Ruby::NoMethod` のメッセージに型を出させる書き方を多用しています。

### ケースを作るときに見つけた注意点

- `steep check --save-expectations` と `--with-expectations` が対象にするファイルは、`check`（`inline: true` なし）と `signature` に一致するファイルだけです（`lib/steep/drivers/check.rb` の `print_typecheck_result`）。`check "...", inline: true` にだけ一致するファイルの診断は、期待値の保存でも比較でも無視されます。そのため inline RBS のケース（`inline_*`）の `Steepfile` では、同じファイルを `check "*.rb"` と `check "*.rb", inline: true` の両方に書いています。両方に一致するファイルは inline のファイルとして検査されます（`Project::Target#possible_source_file?`）。
- rbs collection のケース（`rbs_collection_with_local_source`、`rbs_collection_with_custom_config_path`）は、ネットワークを使わないように `type: local` の source を使っています。`rbs_collection.lock.yaml` は `rbs collection install` で作ったものです。`steep check` は実行のたびに `.gem_rbs_collection/` に絶対パスの symlink を作るので、リポジトリの `.gitignore` で無視しています。
- Steep 2.1.0 は、`__ENCODING__` を含むファイルで内部エラー（`Ruby::UnexpectedError`）を起こし、その診断を書き出すところで再び例外が起きて `steep check` が終わらなくなります。
- `for a, b in ...` のように `for` に複数の変数を書くと、Steep 2.1.0 は内部エラー（`Ruby::UnexpectedError`）を出します。

## 表の見方

| 表記 | 意味 |
| --- | --- |
| smoke `名前` | Steep 2.1.0 の `smoke/名前` で確かめられている |
| e2e `名前` | このディレクトリの `cases/名前` で確かめる |
| 対象外 | `steep check` の結果では確かめられない。理由を添える |

smoke test で確かめられていても、smoke test が確かめていない書き方や組み合わせを e2e のケースで補ったものは「smoke `名前`（e2e `名前` で補強）」と書き、smoke の数に入れています。

## Ruby の診断（`lib/steep/diagnostic/ruby.rb`）

| 診断 | 状態 |
| --- | --- |
| `Ruby::AnnotationSyntaxError` | smoke `diagnostics` |
| `Ruby::ArgumentTypeMismatch` | smoke `diagnostics`、`enumerator`、`literal`、`yield` |
| `Ruby::BlockBodyTypeMismatch` | smoke `diagnostics` |
| `Ruby::BlockTypeMismatch` | smoke `block`、`diagnostics` |
| `Ruby::BreakTypeMismatch` | smoke `block`、`diagnostics` |
| `Ruby::ClassModuleMismatch` | e2e `class_module_mismatch` |
| `Ruby::DeprecatedReference` | e2e `deprecated_reference`（クラス、メソッド、グローバル変数、定数） |
| `Ruby::DifferentMethodParameterKind` | smoke `diagnostics` |
| `Ruby::FallbackAny` | smoke `diagnostics`、`super`、`yield` |
| `Ruby::FalseAssertion` | e2e `false_assertion` |
| `Ruby::ImplicitBreakValueMismatch` | e2e `implicit_break_value_mismatch` |
| `Ruby::IncompatibleAnnotation` | smoke `diagnostics`、`type_case` |
| `Ruby::IncompatibleArgumentForwarding` | e2e `incompatible_argument_forwarding` |
| `Ruby::IncompatibleAssignment` | smoke `hello` ほか多数 |
| `Ruby::InsufficientKeywordArguments` | smoke `class`、`diagnostics` |
| `Ruby::InsufficientPositionalArguments` | smoke `regression` |
| `Ruby::InsufficientTypeArgument` | e2e `insufficient_type_argument` |
| `Ruby::InvalidIgnoreComment` | e2e `invalid_ignore_comment` |
| `Ruby::LibraryRBSError` | e2e `library_rbs_error` |
| `Ruby::MethodArityMismatch` | smoke `diagnostics` |
| `Ruby::MethodBodyTypeMismatch` | smoke `class`、`toplevel` ほか |
| `Ruby::MethodDefinitionInUndeclaredModule` | smoke `const`、`integer`、`regression` |
| `Ruby::MethodDefinitionMissing` | smoke `class`、`diagnostics`、`implements`、`module` |
| `Ruby::MethodParameterMismatch` | smoke `class`、`diagnostics`、`method` |
| `Ruby::MethodReturnTypeAnnotationMismatch` | smoke `diagnostics` |
| `Ruby::MultipleAssignmentConversionError` | e2e `multiple_assignment_conversion_error` |
| `Ruby::NoMethod` | smoke `hello` ほか多数 |
| `Ruby::ProcHintIgnored` | e2e `proc_hint_ignored` |
| `Ruby::ProcTypeExpected` | smoke `diagnostics` |
| `Ruby::RBSError` | e2e `rbs_error_in_ruby_annotation` |
| `Ruby::RedundantIgnoreComment` | e2e `redundant_ignore_comment`、`ignore_comments` |
| `Ruby::RequiredBlockMissing` | smoke `diagnostics` |
| `Ruby::ReturnTypeMismatch` | smoke `diagnostics` |
| `Ruby::SetterBodyTypeMismatch` | e2e `setter_body_type_mismatch` |
| `Ruby::SetterReturnTypeMismatch` | e2e `setter_return_type_mismatch` |
| `Ruby::SyntaxError` | e2e `ruby_syntax_error` |
| `Ruby::TypeArgumentMismatchError` | e2e `type_argument_mismatch` |
| `Ruby::UnannotatedEmptyCollection` | e2e `unannotated_empty_collection` |
| `Ruby::UndeclaredMethodDefinition` | smoke `class`、`implements`、`initialize` ほか |
| `Ruby::UnexpectedBlockGiven` | smoke `diagnostics` |
| `Ruby::UnexpectedDynamicMethod` | smoke `class`、`diagnostics`、`module` |
| `Ruby::UnexpectedError` | e2e `unexpected_error_in_blocks`、`unexpected_error_in_for_loop`、`unexpected_error_from_unknown_type_alias`。メッセージには Ruby の例外と backtrace が入るため、smoke test の runner は範囲と code だけを比べる。`__ENCODING__` のように、出し方によっては `steep check` が終わらなくなるものは対象外 |
| `Ruby::UnexpectedJump` | smoke `diagnostics` |
| `Ruby::UnexpectedJumpValue` | e2e `unexpected_jump_value`（smoke `diagnostics` の `unexpected_jump_value.rb` は `while` の中の `break 10` で、この診断は出ない） |
| `Ruby::UnexpectedKeywordArgument` | smoke `diagnostics` |
| `Ruby::UnexpectedPositionalArgument` | smoke `diagnostics` |
| `Ruby::UnexpectedSuper` | smoke `super` |
| `Ruby::UnexpectedTypeArgument` | e2e `unexpected_type_argument` |
| `Ruby::UnexpectedYield` | smoke `diagnostics`、`yield` |
| `Ruby::UnknownConstant` | smoke `const`、`diagnostics` ほか |
| `Ruby::UnknownGlobalVariable` | e2e `unknown_global_variable` |
| `Ruby::UnknownInstanceVariable` | e2e `unknown_instance_variable` |
| `Ruby::UnknownRecordKey` | smoke `hash` |
| `Ruby::UnreachableBranch` | e2e `unreachable_branch` |
| `Ruby::UnreachableValueBranch` | smoke `type_case` |
| `Ruby::UnresolvedOverloading` | smoke `alias`、`array`、`diagnostics` ほか |
| `Ruby::UnsatisfiableConstraint` | smoke `enumerator`（`diagnostics-ruby-unsat` は Steep の smoke runner で失敗を許されている） |
| `Ruby::UnsupportedSyntax` | smoke `diagnostics`（e2e `unsupported_syntax_variants`、`unsupported_syntax_children_unchecked`、`unsupported_syntax_in_default_configuration` で補強） |
| `Ruby::ElseOnExhaustiveCase` | 対象外。非推奨（`lib/steep/diagnostic/deprecated/`）で、Steep 2.1.0 はこの診断を出さない |
| `Ruby::UnknownConstantAssigned` | 対象外。非推奨で、Steep 2.1.0 はこの診断を出さない |

## RBS の診断（`lib/steep/diagnostic/signature.rb`）

| 診断 | 状態 |
| --- | --- |
| `RBS::SyntaxError` | e2e `rbs_syntax_error`（RBS に構文エラーがあると Ruby のファイルが検査されないことも確かめる） |
| `RBS::DuplicatedDeclaration` | smoke `diagnostics-rbs-duplicated` |
| `RBS::UnknownTypeName` | smoke `diagnostics-rbs`、`unexpected` |
| `RBS::InvalidTypeApplication` | smoke `diagnostics-rbs` |
| `RBS::UnsatisfiableTypeApplication` | e2e `unsatisfiable_type_application` |
| `RBS::InvalidMethodOverload` | smoke `diagnostics-rbs` |
| `RBS::UnknownMethodAlias` | smoke `diagnostics-rbs` |
| `RBS::DuplicatedMethodDefinition` | smoke `diagnostics-rbs` |
| `RBS::RecursiveAlias` | smoke `diagnostics-rbs` |
| `RBS::RecursiveAncestor` | smoke `diagnostics-rbs` |
| `RBS::SuperclassMismatch` | smoke `diagnostics-rbs` |
| `RBS::GenericParameterMismatch` | smoke `diagnostics-rbs` |
| `RBS::InvalidVarianceAnnotation` | smoke `diagnostics-rbs` |
| `RBS::ModuleSelfTypeError` | e2e `module_self_type_error`、`inline_module_self_and_class_alias` |
| `RBS::InstanceVariableDuplicationError` | smoke `diagnostics-rbs` |
| `RBS::ClassInstanceVariableDuplicationError` | smoke `diagnostics-rbs` |
| `RBS::ClassVariableDuplicationError` | e2e `class_variable_duplication` |
| `RBS::InstanceVariableTypeError` | e2e `instance_variable_type_error` |
| `RBS::MixinClassError` | smoke `diagnostics-rbs` |
| `RBS::InheritModuleError` | smoke `diagnostics-rbs` |
| `RBS::RecursiveTypeAlias` | smoke `diagnostics-rbs` |
| `RBS::NonregularTypeAlias` | smoke `diagnostics-rbs` |
| `RBS::InconsistentClassModuleAliasError` | e2e `inconsistent_class_module_alias` |
| `RBS::CyclicClassAliasDefinitionError` | e2e `cyclic_class_alias_definition` |
| `RBS::TypeParamDefaultReferenceError` | e2e `type_parameter_default_reference_error` |
| `RBS::UnsatisfiableGenericsDefaultType` | e2e `unsatisfiable_generics_default_type` |
| `RBS::DeprecatedTypeName` | e2e `deprecated_type_name` |
| `RBS::InlineDiagnostic` | e2e `inline_diagnostic`（未使用の注釈、トップレベルのメソッド定義、注釈の構文エラー） |
| `RBS::UnexpectedError` | 対象外。Steep や rbs の不具合でだけ出る |
| `RBS::VariableDuplicationError` | 対象外。`InstanceVariableDuplicationError` と `ClassInstanceVariableDuplicationError` の親クラスで、これ自体は出ない |

## Ruby の構文（`TypeConstruction#synthesize` の `case node.type` と関連するメソッド）

| 構文（parser gem のノード） | 状態 |
| --- | --- |
| `begin`、`kwbegin` | smoke `kwbegin`、`rescue` |
| `lvasgn`、`lvar` | smoke 多数 |
| `_`、`__any__`、`__skip__` への代入 | smoke `skip` |
| `ivasgn`、`ivar` | smoke `block`、`regression` |
| `match_with_lvasgn`（名前付きキャプチャのある正規表現と `=~`） | e2e `regexp_named_captures_and_special_variables` |
| `op_asgn`（`+=` など。ローカル変数、インスタンス変数、クラス変数、グローバル変数、属性、`[]`） | e2e `operator_assignment` |
| `or_asgn`、`and_asgn`（`\|\|=`、`&&=`） | e2e `or_and_assignment` |
| 定数への `\|\|=`、属性や `[]` への多重代入 | e2e `rare_assignment_targets` |
| `super`（引数あり） | smoke `super`、`class` |
| `zsuper`（引数なし） | smoke `super` |
| `def` | smoke 多数 |
| `defs`（`def self.foo`） | smoke `class`、`implements` |
| 1 行のメソッド定義（`def foo = 1`） | e2e `endless_method_definition` |
| `return` | smoke `diagnostics` |
| `break` | smoke `block`、`diagnostics` |
| `next`（ブロックの中） | e2e `next_in_block` |
| `retry` | e2e `retry_in_rescue` |
| `procarg0`、`mlhs`（ブロック引数の分解） | smoke `hash`、`regression` |
| `arg`、`optarg`、`restarg`、`kwarg`、`kwoptarg`、`kwrestarg`、`blockarg` の型 | e2e `method_parameter_types` |
| 末尾の必須引数（`def foo(a, *b, c)`） | e2e `trailing_positional_parameter` |
| `blockarg` と `block_pass` | smoke `block`、`block_given`、`regression` |
| 名前のない `*`、`**`、`&` の受け渡し | e2e `hash_shorthand_and_anonymous_parameters` |
| `forwarded_args`、`forward_arg`（`...`） | e2e `argument_forwarding`、`incompatible_argument_forwarding` |
| `int`、`str`、`sym`、`true`、`false`、`nil` | smoke `literal` |
| `float`、`rational`、`complex` | e2e `numeric_and_range_literals` |
| `irange` | smoke `regression`（e2e `numeric_and_range_literals` で補強） |
| `erange`、終わりや始まりのない範囲 | e2e `numeric_and_range_literals` |
| `dstr` | smoke `dstr` |
| `xstr`、`dsym`、ヒアドキュメント | e2e `interpolated_symbol_and_command_string` |
| `%w`、`%i`、`?a`、文字列の連結 | e2e `string_formats` |
| `__FILE__`、`__LINE__`、`__dir__`、`__method__` | e2e `source_location_keywords` |
| `__ENCODING__` | 対象外。Steep 2.1.0 が内部エラーを起こし、`steep check` が終わらない |
| `regexp`、`regopt` | smoke `regexp` |
| `nth_ref`、`back_ref`（`$1`、`$~` など） | e2e `regexp_named_captures_and_special_variables` |
| `hash`、`kwargs` | smoke `hash` |
| ハッシュの値の省略（`{ name: }`） | e2e `hash_shorthand_and_anonymous_parameters` |
| `array` | smoke `array` |
| 配列リテラルの `splat` | e2e `array_literal_with_splat` |
| メソッド呼び出しの `splat`、`kwsplat` | e2e `call_with_splat_and_keyword_splat` |
| `class`、`module` | smoke `class`、`module` |
| `sclass`（`class << self`） | e2e `singleton_class_definition` |
| `self` | smoke `self` |
| `const`、`cbase`、`casgn` | smoke `const` |
| `yield` | smoke `yield` |
| `and` | smoke `and` |
| `or`（`\|\|` と `or`） | e2e `narrowing_with_and_or` |
| `if` | smoke `if` |
| `unless`、`elsif`、三項演算子、後置の `if` | e2e `conditional_forms` |
| `case`（`when`） | smoke `case`、`type_case` |
| `case_match`（`case`/`in` のパターンマッチ） | e2e `unsupported_syntax_variants`（`Ruby::UnsupportedSyntax` になる）、`unsupported_syntax_children_unchecked` |
| `match_pattern`（`=>`）、`match_pattern_p`（`in`）、`forwarded_restarg`（`*` の転送） | e2e `unsupported_syntax_children_unchecked`（`Ruby::UnsupportedSyntax` になり、中の式は型検査されない）、`unsupported_syntax_in_default_configuration`（default の preset では何も表示されない） |
| `forwarded_kwrestarg`（`**` の転送） | e2e `unsupported_syntax_children_unchecked`（hash の要素に書くと、Steep 2.1.0 は `type_hash` の `raise` で内部エラー（`Ruby::UnexpectedError`）を出す） |
| `rescue`、`resbody`、`ensure` | smoke `rescue`、`ensure` |
| `rescue => e` の変数の型 | e2e `rescue_exception_variable` |
| `begin`/`rescue`/`else`/`ensure` の値 | e2e `begin_end_and_ensure_values` |
| `masgn`（多重代入） | smoke `regression`（`*` だけ）、e2e `multiple_assignment` |
| `for` | e2e `for_loop` |
| `for` に複数の変数を書く形 | e2e `unexpected_error_in_for_loop`（Steep 2.1.0 は内部エラー（`Ruby::UnexpectedError`）を出す） |
| `while`、`until`、`while_post`、`until_post`、後置の `while`/`until` | e2e `while_and_until_loops` |
| `loop` と `break` の値 | e2e `loop_with_break_value` |
| `defined?` | e2e `defined_keyword` |
| `gvasgn`、`gvar` | e2e `global_variables` |
| `cvasgn`、`cvar` | e2e `class_variables` |
| `alias` | e2e `alias_keyword` |
| `send`、`block` | smoke 多数 |
| `csend`（`&.`） | smoke `tsort`（e2e `safe_navigation_operator` で補強） |
| private メソッドの呼び出し | e2e `private_method_call` |
| `numblock`（`_1`） | e2e `numbered_block_parameters` |
| `itblock`（`it`） | e2e `it_block_parameter` |
| `lambda`（`->`） | smoke `lambda`、`regression` |
| `Kernel#lambda`、`Kernel#proc` | e2e `kernel_lambda_and_proc` |
| `assertion`（`#:` による型の表明） | e2e `type_assertion`、`false_assertion`、`type_assertion_with_void_and_self`（`void`、`self`、`instance`、`class` を書いたもの） |
| `tapp`（`#$` による型引数の指定） | e2e `type_application`、`unexpected_type_argument`、`insufficient_type_argument`、`type_argument_mismatch` |
| `redo`、`preexe`（`BEGIN`）、`postexe`（`END`）、`undef`、フリップフロップ | e2e `unsupported_syntax_variants` |
| `class <<` に式を渡す形 | smoke `diagnostics` |

## 型の推論と絞り込み（`lib/steep/type_inference` など）

| 項目 | 状態 |
| --- | --- |
| `is_a?`、`kind_of?`、`instance_of?` による絞り込み | e2e `narrowing_with_is_a` |
| `nil?` と `!` による絞り込み | e2e `narrowing_with_nil_check` |
| `case`/`when` のクラスによる絞り込み（`Class#===`） | smoke `type_case` |
| リテラルとの `==`、`===` による絞り込み | smoke `narrowing-else`、`type_case`（e2e `narrowing_with_literal_equality` で補強） |
| `Module#<`、`Module#<=` による絞り込み | e2e `narrowing_with_module_comparison` |
| `&&`、`\|\|`、`and`、`or` による絞り込み | smoke `and`（e2e `narrowing_with_and_or` で補強） |
| `return`、`raise` の後の絞り込み | e2e `narrowing_after_early_return` |
| `%a{pure}` のメソッド、`Array#[]`、`Hash#[]` の結果の絞り込み | e2e `narrowing_with_pure_method_calls` |
| 条件の中の代入による絞り込み | smoke `hash` |
| `block_given?` による絞り込み | smoke `block_given`、`module` |
| `while` の条件による絞り込み | e2e `while_and_until_loops` |
| 到達しない分岐の検出 | smoke `type_case`、e2e `unreachable_branch` |
| `Array#compact` の特別扱い | smoke `compact` |
| `Hash#compact` の特別扱い | e2e `hash_compact` |
| ジェネリクスの型引数の推論 | smoke `map`、`enumerator`、`hash`（e2e `generic_class_inference` で補強） |
| オーバーロードの解決 | smoke `alias`、`method` |
| ブロックやラムダへの型の伝搬 | smoke `lambda`、`tsort`、`block` |
| 多重代入の型 | e2e `multiple_assignment` |
| union、optional | smoke `if`、`array` ほか |
| intersection | e2e `intersection_types` |
| tuple、record | smoke `alias`、`hash` |
| record の省略できるキー（`?age:`） | e2e `record_with_optional_key` |
| literal 型 | smoke `literal` |
| interface 型 | smoke `hello`、`stdout`、`interface` |
| `self`、`instance`、`class` 型 | smoke `class`（e2e `self_instance_and_class_types` で補強） |
| `singleton(...)` 型 | e2e `singleton_types` |
| `void`、`bot`、`bool` | e2e `void_bot_and_boolean_types` |
| `untyped`、`top` | e2e `untyped_and_top_types` |
| 型エイリアス、ジェネリックな型エイリアス | smoke `alias`、`regression` |
| rbs gem が名前を解決できず相対名のまま残した型エイリアス（展開するときに `::` から探す） | e2e `relative_type_alias_name_left_unresolved` |
| module の self type（`module A : _Each`） | smoke `module` |
| 既存クラスへのメソッド追加、`...` による overload の追加、`prepend` | smoke `extension`、`regression` |
| `%a{implicitly-returns-nil}` と `implicitly_returns_nil!` の有無 | e2e `implicitly_returns_nil_annotation` |
| `%a{deprecated}` | e2e `deprecated_reference`、`deprecated_type_name` |

## 注釈

| 注釈 | 状態 |
| --- | --- |
| `@type var` | smoke 多数（e2e `annotation_type_var_in_block_scope` で補強） |
| `@type method` | smoke `diagnostics`、`method` |
| `@type const` | smoke `const` |
| `@type ivar` | smoke `diagnostics`、`regression` |
| `@type return` | smoke `diagnostics` |
| `@type break` | smoke `block` |
| `@type self` | smoke `self`、`toplevel_annotations` |
| `@type block` | e2e `annotation_type_block` |
| `@type instance` | e2e `annotation_type_instance_and_module` |
| `@type module` | e2e `annotation_type_instance_and_module` |
| `@dynamic` | smoke `class`、`module`（e2e `annotation_dynamic_singleton_method` で補強） |
| `@implements` | smoke `class`、`implements`、`method`（e2e `annotation_implements_generic_module` で補強） |
| `#:`（型の表明） | e2e `type_assertion` |
| `#$`（型引数の指定） | e2e `type_application` |
| `# steep:ignore`、`# steep:ignore 診断名`、`# steep:ignore:start` と `# steep:ignore:end` | e2e `ignore_comments` |
| 不正な `steep:ignore` | e2e `invalid_ignore_comment` |
| 不要な `steep:ignore` | e2e `redundant_ignore_comment` |
| inline RBS: `#:` と `# @rbs` によるメソッドの型、引数ごとの `# @rbs`、`*`、`**`、`&` | e2e `inline_method_types` |
| inline RBS: overload、`...`、末尾の `#:` による返り値の型 | e2e `inline_overloads_and_return_annotation` |
| inline RBS: `attr_*` の `#:`、`# @rbs @ivar` | e2e `inline_attributes_and_instance_variables` |
| inline RBS: 定数、`include` | e2e `inline_constants_and_mixins` |
| inline RBS: superclass の型引数（`#[String]`） | e2e `inline_generic_superclass` |
| inline RBS: `module-self`、`#: class-alias` | e2e `inline_module_self_and_class_alias` |
| inline RBS: `# @rbs skip` | e2e `inline_skip_annotation` |

## Steepfile の設定

| 設定 | 状態 |
| --- | --- |
| `target`、`check`、`signature`（glob） | smoke 全体 |
| `check`、`signature` にディレクトリを渡す | e2e `target_group`、`ignored_paths`、`multiple_targets` |
| 複数の target と、target 間での RBS の共有 | smoke `diagnostics-rbs`（e2e `multiple_targets` で補強） |
| `unreferenced!` | smoke `diagnostics-rbs` |
| `group` | e2e `target_group` |
| `ignore`、`ignore_signature` | e2e `ignored_paths` |
| `check ..., inline: true` | e2e `inline_*` |
| `implicitly_returns_nil! true` | smoke 全体 |
| `implicitly_returns_nil! false` | e2e `implicitly_returns_nil_annotation` |
| `configure_code_diagnostics(Ruby.all_error)` | smoke 全体 |
| `configure_code_diagnostics(Ruby.default)` | e2e `diagnostic_configuration_default` |
| `configure_code_diagnostics(Ruby.strict)` | e2e `diagnostic_configuration_strict` |
| `configure_code_diagnostics(Ruby.lenient)` | e2e `diagnostic_configuration_lenient` |
| `configure_code_diagnostics(Ruby.silent)` | e2e `diagnostic_configuration_silent` |
| `configure_code_diagnostics` を呼ばない場合（`Ruby.default` になる） | e2e `diagnostic_configuration_omitted` |
| `configure_code_diagnostics` のブロックで診断ごとの重さを変える、`nil` で消す | e2e `diagnostic_configuration_customized_by_block` |
| 診断の重さ（error、warning、information、hint）と、既定の `--severity-level=warning` による絞り込み | e2e `diagnostic_configuration_*` |
| `library`（標準ライブラリ） | smoke `tsort`（e2e `standard_library` で補強） |
| `repo_path` | e2e `repository_path_library` |
| rbs collection（`rbs_collection.yaml` と `rbs_collection.lock.yaml`） | e2e `rbs_collection_with_local_source` |
| `disable_collection` | e2e `rbs_collection_with_local_source` |
| `collection_config` | e2e `rbs_collection_with_custom_config_path` |
| `stdlib_path` | 対象外。core と stdlib の場所を環境ごとの絶対パスで指定する設定で、ケースに固定できない |
| git の source を使う rbs collection | 対象外。取得にネットワークが必要。読み込みの仕組みは local の source のケースで確かめる |

## `steep check` では確かめられないもの

| 項目 | 状態 |
| --- | --- |
| hover（`Services::HoverProvider`） | 対象外。LSP の機能で、`steep check` の結果に出ない |
| 補完（`Services::CompletionProvider`） | 対象外。LSP の機能 |
| signature help（`Services::SignatureHelpProvider`） | 対象外。LSP の機能 |
| 定義や実装への移動（`Services::GotoService`） | 対象外。LSP の機能 |
| workspace symbol、document symbol | 対象外。LSP の機能 |
| `steep watch`、`steep langserver`、daemon | 対象外。`steep check` 以外のコマンドや実行方法 |
| `steep stats`、`steep annotations`、`steep init`、`steep project`、`steep binstub` | 対象外。`steep check` 以外のコマンド |
| `steep check` の `-e`、`--severity-level`、`--group`、`--validate`、`--format`、パスの指定、`--jobs` | 対象外。smoke test の実行方法ではコマンドラインが固定されている。重さの絞り込みそのものは既定値の `warning` で `diagnostic_configuration_*` が確かめる |

## 集計

| 区分 | 項目数 | smoke | e2e（このディレクトリで新たに確かめる） | 対象外 |
| --- | --- | --- | --- | --- |
| Ruby の診断 | 60 | 35 | 22 | 3 |
| RBS の診断 | 30 | 17 | 11 | 2 |
| Ruby の構文 | 73 | 31 | 40 | 2 |
| 型の推論と絞り込み | 33 | 18 | 15 | 0 |
| 注釈 | 24 | 9 | 15 | 0 |
| Steepfile の設定 | 24 | 6 | 16 | 2 |
| `steep check` では確かめられないもの | 8 | 0 | 0 | 8 |
| 合計 | 252 | 116 | 119 | 17 |

`cases/` には 112 のケースがあります。1 つのケースが複数の項目を確かめることがあるので、ケースの数と e2e の項目数は一致しません。
