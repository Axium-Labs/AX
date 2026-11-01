# AX-Bench Phase 1 — Runtime Microbenchmark

```json
{
  "schema_version": 1,
  "fixture_version": 1,
  "timestamp_utc": "2026-11-01T17:04:22.515293+00:00",
  "git_commit": "54bcce9e17be15ed09d97e5dc1d496f89c4bf283",
  "git_status": "M README.md\n M README.zh-CN.md\n M crates/cli/src/commands/prompt_execution_tests.rs\n M crates/core/src/execution_tests.rs\n M crates/core/src/tests.rs\n M crates/mcp/tests/runtime_sandbox.rs\n M crates/tool/tests/discovery.rs\n M docs/README.md\n M test/evolution/lifecycle.rs\n M test/evolution/persistence.rs\n M test/harness/frontend.rs\n M test/harness/workspaces.rs",
  "tracked_diff_sha256": "3d05761595f1c8036ae53f3800b87e3ad104dc487dadceb58e8862c490af12f5",
  "rust": "rustc 1.92.0 (ded5c06cf 2025-12-08)\nbinary: rustc\ncommit-hash: ded5c06cf21d2b93bffd5d884aa6e96934ee4234\ncommit-date: 2025-12-08\nhost: x86_64-pc-windows-msvc\nrelease: 1.92.0\nLLVM version: 21.1.3",
  "os": "Windows-11-10.0.26300-SP0",
  "cpu": "Intel64 Family 6 Model 183 Stepping 1, GenuineIntel",
  "logical_cpus": 32,
  "build": "release opt-level=3 lto=thin codegen-units=1 strip=symbols; offline locked; default features",
  "rustflags": "",
  "cargo_encoded_rustflags": "",
  "python": "3.14.7",
  "tokio_workers": 4,
  "parameters": {
    "rounds": 3,
    "samples": 20,
    "warmup": 5,
    "sizes": [
      16,
      128,
      1024
    ],
    "timeout": 120,
    "output": "C:\\Users\\14181\\Desktop\\axlab\\ax\\benchmarks\\runtime\\results\\final-baseline-2026-11-02"
  },
  "sha256": {
    "benchmarks\\runtime\\Cargo.lock": "9b7cdad760e3c43aa67717de60e09d7d730d1e49ea302071deb37f012da4ce60",
    "Cargo.lock": "fb2128b2608fd991df31da034e6d2c6d7fa626c197b3934fba6a8689595b13db",
    "benchmarks\\runtime\\src\\main.rs": "56dd643ce72114a4b6e9c20a2e2148e4f769e049f73400c9c8d9e10d61add966",
    "benchmarks\\runtime\\run.py": "880116ebe88483f6446c728e47f68072722bf18d39dc96f4db715ced1c4d6619",
    "benchmarks\\runtime\\datasets\\message.txt": "fdae7dc7c685f9496b437854fdc02cd20b222fa22d54eb5e4c600ae80b22597a",
    "target\\release\\ax-runtime-bench.exe": "88e37755de2e7efac4bb0af4a812d23b0b530088e7d485d9ba98338c4ff48326",
    "target\\release\\ax.exe": "2a581e2634feb02d99921baa58eeb7b220ffb2004a0e9243386da25cc0a84988"
  },
  "hardware": "{\"CPU\":{\"Name\":\"13th Gen Intel(R) Core(TM) i9-13900HX\",\"NumberOfCores\":24,\"NumberOfLogicalProcessors\":32},\"RAM\":34074894336}",
  "production_source_sha256": {
    "crates\\cli\\Cargo.toml": "40dbb94dca56ac1c9b46c0da9df35f562cb4ddb0d6510b065404afe73c5ae20c",
    "crates\\cli\\src\\acp.rs": "1ecef09ed229d93a8e93828fcd4d3ad95867347ec463006086541d7d2622d968",
    "crates\\cli\\src\\acp_subagents.rs": "8e23f0880a182055d7b16735948a4f0b829744edd60fbcb354864bc9b9882d4f",
    "crates\\cli\\src\\acp_workspace.rs": "fbded7ac5db7413c3338aa6dc98fe799fd9601389e8a9e85d15c500c6e04f75c",
    "crates\\cli\\src\\app.rs": "5e8dd159ddb2f5e6adcebd4f2709996cae7f377a5d0c2ec7e1c4fca6970c8b39",
    "crates\\cli\\src\\args.rs": "661ca79f360bf4b7566ddaef91ec072766c7934d250e2564548e1731a82f3691",
    "crates\\cli\\src\\auth_login.rs": "d1882f4014e6cdd32ad111f8694c4423611dff175961c3669a7f5c3f12645da6",
    "crates\\cli\\src\\bootstrap.rs": "96d79e00cd4fc214c15fe0cf52bbbadd6f176cc6d6f72d66a6dfa99955df4f7d",
    "crates\\cli\\src\\capabilities.rs": "0241febee6563db5a5f824f3ab6d2f4cf0d1d1a0aee8de2ffff035b6e00bfe28",
    "crates\\cli\\src\\capability_import.rs": "7537010db606828542c880e3edc9b6a67e598aca311026300205e59d0689e99a",
    "crates\\cli\\src\\child_benchmark_tests.rs": "53e1dc78743e95acc624488320e3af4c920d372e69a4c189dc9360719192fb77",
    "crates\\cli\\src\\child_file_baseline.rs": "312fb14a40961fe969cad9c4a9b24143129527638483782dd1e13a0de70bf1b9",
    "crates\\cli\\src\\child_runtime.rs": "c6d56fb500e00da06f8869b56e3ab8ef3d7abbac42e059723fb009dcba0fc033",
    "crates\\cli\\src\\child_runtime_tests.rs": "830bce2a0852aadc02c2a8691d3bf30249f0f2c8fd9df2a8a0f0f501e51997aa",
    "crates\\cli\\src\\child_workspace.rs": "6fd7fe0fc9f1b3c766ab244275ffd6c5e795fc7005f11004dfc1ba7226a1398a",
    "crates\\cli\\src\\commands\\agents.rs": "fa6f44ad9f294ef5909870a5132fc03aea6e17d89c748361c5b5cd54ee28a384",
    "crates\\cli\\src\\commands\\backup.rs": "0fa96d69c5fb573e93e226e26d9dfe4779f5ed7ada5afa026568c0fd6a99dafb",
    "crates\\cli\\src\\commands\\evolution.rs": "741c7f98b0cf306e2d9ab572d3bfa75b06a4f8317e92706a085520b775a35348",
    "crates\\cli\\src\\commands\\goal.rs": "6aced5288d0e399de9ac26859d023c9f2a53e04dcb0055a618056e735bfb5b6d",
    "crates\\cli\\src\\commands\\mod.rs": "d758cea38026dcfc3300e4f8f229187bca621195953e4c3f120a8c3cf4a1a634",
    "crates\\cli\\src\\commands\\prompt_execution_tests.rs": "b7efa9850105c9a45d66baa6597dcc2edfaa88c43e3eeed5ced48b9c6dfbf22a",
    "crates\\cli\\src\\commands\\run.rs": "78624a8f80c495d059f8b5984d738f671efe614a6b3b487c05c39c24a6a5d1ab",
    "crates\\cli\\src\\computer_use.rs": "b8f1f3f66b924c38a0500ee5e7393ab0e2610cc906e08dd683eccf0b9056973b",
    "crates\\cli\\src\\config.rs": "01feb4a3ede0e4c73eaa6024f417f352a7f2a8f311a9100bf161ddca064ce69f",
    "crates\\cli\\src\\crew_device.rs": "3dd5efeeeaa8c89a4dca3a659c89d861d4a5f0995d92d7585039b67f8e89ccec",
    "crates\\cli\\src\\distributed_client.rs": "3cb4518147dec9bfa373dbf4d6d38d95562725d0c203d736d05a1348c67288a8",
    "crates\\cli\\src\\distributed_goal.rs": "247080920129b35e5f60d76b1b4ce678a004ad65433cb17aea48d345924d1d66",
    "crates\\cli\\src\\distributed_host.rs": "768908dbd65afe629ebd98cdbbea2b2b03a8299ff022d0581504fde4a322138c",
    "crates\\cli\\src\\distributed_session.rs": "f650b16508fabe3b739ef266640513877e83c6a293d350a3bf8828b33f683fe8",
    "crates\\cli\\src\\distributed_settings.rs": "2f078673c4d4c620d1f69d3da1371089451bd096093970627d2679ea23199737",
    "crates\\cli\\src\\distributed_tool.rs": "0488dbc991b50362d3fa8cc998773195246e80cdc3e3b7e4a055b1e00d520745",
    "crates\\cli\\src\\distributed_worker.rs": "bf87fca18a608f453f2c95834b979d0bc0812fcfb313c0704d35d5b6d8830d0d",
    "crates\\cli\\src\\e2e_tests.rs": "5fa6945fb0a9fea333a044cfdf3c00f25bc9a131aab815a1ecd4382b06fec5b7",
    "crates\\cli\\src\\evolution.rs": "9485c70736171f42160d39a966057ef44bf3cea4b20f5d7b4092fe6eeea05c5a",
    "crates\\cli\\src\\execution.rs": "2946726ebe798c1634ca0d87492845ec32713b3659327138d2369c5653b4f62a",
    "crates\\cli\\src\\file_reference.rs": "8eb98aa00ecc473c4783ac23461b65a47627d8f0f36ec33c5267192dab50aca3",
    "crates\\cli\\src\\goal_settings.rs": "e4059a1a06dea41676a66d394e93c6ecbc3b0eb7ff1134fbdf30c2cc8edc63f9",
    "crates\\cli\\src\\host_permissions.rs": "9b45cd798f14da56882742cc9f9786170128875cdcee38be5978674478457b84",
    "crates\\cli\\src\\long_project.rs": "1e1b958e22eddf98deb3ac7123decbee65c8de27b15259836f31c40860f05ed4",
    "crates\\cli\\src\\main.rs": "258d844fd8809520c812d739668fabc9e3b2a1cfd2908b13397ac91cbb279967",
    "crates\\cli\\src\\memory_context.rs": "93eddc09932edb24b5c643a5af294403b261aabbc412c4b9f7cafc35ff7e17f3",
    "crates\\cli\\src\\memory_tool.rs": "b6ceee7f0ff39b43608a475cb8601a5fb45717507990978ae5f1ba4c7bcf63bd",
    "crates\\cli\\src\\model_selection.rs": "4a0c795afdc223b291feec05d43c975bdf9ef429e734660dec6d02258f552a7f",
    "crates\\cli\\src\\mods\\catalog.rs": "6747d80d3294b6e2e042357552a32dd6e748c37dbdd46a3f70127489d40102bc",
    "crates\\cli\\src\\mods\\host.rs": "b3acb243296a25bb42b0cb7ab605c6ed45cecb2e57e96b91a3daf4a331814fd3",
    "crates\\cli\\src\\mods\\mod.rs": "8488de7ec93b97438ea7a48267c09a04f29f50962599a92e3ca9569d7ddf909f",
    "crates\\cli\\src\\mods\\tools.rs": "1daf397b02e866d3abe76856cb46b076540490d16ec4784dfedf02481cfa01b4",
    "crates\\cli\\src\\personalization.rs": "9a74258ecbb05360a7b60a564ec976a779f2bc8edf17c8a3dfa6f30b9896f3b2",
    "crates\\cli\\src\\project_identity.rs": "020f2e49854c3a3639a1eba3b2c1af9d6aa723c0618c00c385531273b5780917",
    "crates\\cli\\src\\project_instructions.rs": "ef4aa022b919fb0ce4bab5a2626feb9768e6e725784ae3aa4117deacbb314337",
    "crates\\cli\\src\\providers.rs": "e8bb623dee93bfbc33fbb2ce1bc6887119ef7d136c1fd868ae0d548c602a5469",
    "crates\\cli\\src\\repl\\mod.rs": "1e99827463cb58fa8b288abb05b726c1ffa409ecbd419a9924598624496a43d4",
    "crates\\cli\\src\\repl\\state.rs": "b7e6620905ad2ee4c318d1fd2468ed7a889ad28126ea410211a306f36fa0a869",
    "crates\\cli\\src\\runtime\\builder.rs": "53bd38c1bc0c0be38b6143fc4edb333d07634c0ebd2c648478be2432d9464159",
    "crates\\cli\\src\\runtime\\mod.rs": "eb711c384620ae52a481d4d05db2479b9307752e352a95a9a8a4f53f974b33d9",
    "crates\\cli\\src\\session_projects.rs": "45f906d8a688eef912438a2a7358073eac22ebfaedda8f331ccfcebde4bac76d",
    "crates\\cli\\src\\session_restore.rs": "2d0cd4c7c07fa06291b044795c423be837fda19bea93e06a4d1dcfc6620bac6f",
    "crates\\cli\\src\\skill_invocation.rs": "3e303dad94288cba17925669d616049d0b392448df4e6fcf1d03716604244c4a",
    "crates\\cli\\src\\skill_settings.rs": "f463e400877fd4ae800b7d08dde813b3b044ec3b3a59e407211feee3c58dd37e",
    "crates\\cli\\src\\storage_location.rs": "3652124d1052b3b3a9903fd4a5a31a814f7d3b43e815644a26bc6be25b5c9ccd",
    "crates\\cli\\src\\subagent_settings.rs": "79cc36c194c253ee65827070a7e530a6215d2dadbde4ef94aad6380c6763c6a2",
    "crates\\cli\\src\\system_info.rs": "edd91d8d15524c50e5db095512ab38dadbe1460bba365d3dd3278fdd99d5f734",
    "crates\\cli\\src\\tui\\bottom_pane\\approval_dialog.rs": "d6fddf51cee5df6f93dbca54a68e4f79036c2f43b475d23c6947c38223c4de8f",
    "crates\\cli\\src\\tui\\bottom_pane\\composer.rs": "61eac4636560cbdf404dc05ae9c952655530fef2e2c5012a3ace95e8710fa493",
    "crates\\cli\\src\\tui\\bottom_pane\\evolution_review.rs": "717543f8550a524ace7d856ac444c09a8c3e373d017a23c304c86897e1b5bf84",
    "crates\\cli\\src\\tui\\bottom_pane\\file_popup.rs": "ac0aa14a7691bf5d543e856897eec8d632faebb20f4a74d409ef969ff60781cc",
    "crates\\cli\\src\\tui\\bottom_pane\\memory_editor.rs": "9ccea5d60f3967d7302a11e89b67ebe3cbf4ab47aacf5236e6f621fe036f222b",
    "crates\\cli\\src\\tui\\bottom_pane\\mod.rs": "c1513146544c29da7c854988fb2d3e817c3c0ffdb8ca28b2895615ebc9621406",
    "crates\\cli\\src\\tui\\bottom_pane\\model_picker.rs": "e9df89dd89dec3f085ef32245b6e6f04bf0fa1f0f97325c5ed2c0f251ae34370",
    "crates\\cli\\src\\tui\\bottom_pane\\picker.rs": "2f775e02b0178bf0a456ced085838d606206cd84a110e66367defcbd036bd9e1",
    "crates\\cli\\src\\tui\\bottom_pane\\secret_input.rs": "1c70f599c3caa5c3d79572f48f0add36179f989df79b117c3b255553d16217dd",
    "crates\\cli\\src\\tui\\bottom_pane\\session_picker.rs": "15bc6a3e3a930a3175533d2aa3553c7ddb975a3b80f66de906d38844076ce68a",
    "crates\\cli\\src\\tui\\bottom_pane\\session_rename.rs": "3e9e80ba8d3f551077260fcbb2643ca53e7ab22dd8c6da62b13f3d59d4267e36",
    "crates\\cli\\src\\tui\\bottom_pane\\slash_popup.rs": "155d637025f64571fd0a8fdf75068437316afe073cdf738fa7b15689a91a1900",
    "crates\\cli\\src\\tui\\bottom_pane\\status_line.rs": "7c64a5adeb204d43970eec8e5b09f1dbba1356af140b677b828fa850a0fba77d",
    "crates\\cli\\src\\tui\\bottom_pane\\surface.rs": "9a819378c39bb5c6bcb771c4fc44fef9a6ed0f470b98c2989ecbbfd6ccb3b95d",
    "crates\\cli\\src\\tui\\bottom_pane\\textarea.rs": "dcf25ca0985523dbfd284d93771438e919b3c5ebc3ba2f69c5cec72baae409cd",
    "crates\\cli\\src\\tui\\bottom_pane\\view.rs": "e6b47102d8a07ee7e07cf1b4bc9e67558908f2128b47ea4917587afc1aa1b324",
    "crates\\cli\\src\\tui\\catalog_refresh.rs": "1e4580e19ef7833f321a540390516875e7ae891797a29959d35827f8fe6421f5",
    "crates\\cli\\src\\tui\\commands\\catalogs.rs": "cd1bb9a178bb9dbc645447407b5b02ad6e82ba78e8d4d9c49766418467926e77",
    "crates\\cli\\src\\tui\\commands\\memories.rs": "7a2fa13eb5d34a2b7ffbf494986d69e813e7c319983541fb2b4bb16136c35ece",
    "crates\\cli\\src\\tui\\commands.rs": "ecb3bfd584cdcdd2b49167f712b00de5d32dba5cd24ec5f05cb3574719256832",
    "crates\\cli\\src\\tui\\markdown.rs": "fa0dfa3d16dbe14d030fb52de6fd59fc0ff629299794c68855c79177f08b7462",
    "crates\\cli\\src\\tui\\mod.rs": "adbb855ee4bedea7bf8a9b75b0b1acb696ec5398ea625b62ea8cd7c55aea57c9",
    "crates\\cli\\src\\tui\\startup.rs": "dc486511b9cbe2aee8af652c5f562049f8af9b0027989b75b18675d79b7e417d",
    "crates\\cli\\src\\tui\\terminal_backend.rs": "7be41fed68c962247e3f4762d47cb83d0d07ee7de9a6051abf0d91b9ab592c00",
    "crates\\cli\\src\\tui\\theme.rs": "309fd997ce9c13f76c8c4d2fdb49d839eefafc1fbdb6712868c035e02cf5029d",
    "crates\\cli\\src\\tui\\transcript.rs": "889eeb4ce5e790c79080f7517b9c0fe2a19eead09815faa5fade5aeee9044d08",
    "crates\\cli\\src\\update.rs": "f164c47860a2404dbd14854c868ccd2d0803b9f658a65c8e09456f39f4ee0bc9",
    "crates\\cli\\src\\worktree_changes.rs": "6ca78274791102c7d66c06124190497b59ec48413e58b1f2a5ce0405649de369",
    "crates\\core\\Cargo.toml": "c782a1b0d245276ba42ce1bbd525ea65dd40e4c507ac978784abc808bed905e7",
    "crates\\core\\src\\approval.rs": "a95290a841bbc65f4f4501698a7cd4df879021b8ab165d4eeade64e5cebc2fec",
    "crates\\core\\src\\budget.rs": "e1857f34508fd7472bf94f0ff19341e4266a2d30cef5389c45e3ff9070839d03",
    "crates\\core\\src\\child.rs": "823baa1b44cbbae20f28fa1cc58109e1b224602f69c060b42caae2064e266191",
    "crates\\core\\src\\child_dispatch.rs": "1558051ac4a00c04f578cca52396a2ac79ad79c00f231ac7ad67e215a4099f4e",
    "crates\\core\\src\\child_dispatch_tests.rs": "17eb26f727427ffa36a47a9e582ece121efd53373caa65da2b7445a88dc63e24",
    "crates\\core\\src\\child_policy.rs": "34ca87c73f17b53e2e68089c79771940d56f5538f77ad00186707f5743797718",
    "crates\\core\\src\\child_result.rs": "01cea6ed26d1df255080438b66ae108e7a75387c8c960e50bdb40f0b53fe7d16",
    "crates\\core\\src\\child_result_tests.rs": "06a189fc761cf0b3f9d5dbeefcaa51795156c7e6724ac592caaf73ef6eeacc22",
    "crates\\core\\src\\child_tests.rs": "f6051e3c95651c3ac35267404de81f782380fde20e8f4e7006804e8ab1c3315b",
    "crates\\core\\src\\compression\\mod.rs": "d0cf5a508f0581f8f888d53aa02f1df39791f3685e97687f6fb7a27784b9880d",
    "crates\\core\\src\\compression\\pipeline.rs": "fd89373f320ed68b0660af9c2d8d61bd3991dd579aa0ff15739c8de73301e08e",
    "crates\\core\\src\\compression\\summary.rs": "4e4441613a4170fdfc8160fe19c61831261dbbc96bd3448ea2f4a0c401a2ff5f",
    "crates\\core\\src\\context.rs": "ad5f9e17ed587e8a9cc70d204a2df66232f04dae684812c9196a61cd2f700038",
    "crates\\core\\src\\continuation.rs": "4cc01f4d2740987d228c326545c1d4d7fe75841e2355763b158c4ffa57fbc545",
    "crates\\core\\src\\error.rs": "a905df57a3ae00c1ae1f35d4e408b442357b616ded991f94733fc12d25354bff",
    "crates\\core\\src\\event.rs": "d5585f630b23959cc0fbf98e64cd6a02afd605a0c1ed68a01b422ec84ffa4fc0",
    "crates\\core\\src\\execution.rs": "20756c86d03c29b5cd587eebc1ae60eae35973da6bf1cc15ae12d9906a281e13",
    "crates\\core\\src\\execution_tests.rs": "7781e4c0f9ca1c5a1c64775a5c1bc79da9e143d98a9f75d40110d384cb0d4061",
    "crates\\core\\src\\extension.rs": "dd6b4d544ae1fbcb59fc1a1f7f58ad2c927e78a3dc4ee8afacd28874e9216803",
    "crates\\core\\src\\goal\\validation.rs": "a12e3e5630e1a8855dd07c6c748907152d9683548f9998e55669a76731128a63",
    "crates\\core\\src\\goal.rs": "d82b49e298c82f12ae3daea2d27d7f3533542846a3d4ae823be7e08b5212b53d",
    "crates\\core\\src\\harness.rs": "07329b4f92cb10eeca5400a1f4c49376a90677d7105270d3f51530ff51eaa527",
    "crates\\core\\src\\instructions.rs": "3cfbac8e84d5be9c4492df48746d71cd6eb35adefb1d1128f9c2a9c557f20ae4",
    "crates\\core\\src\\instructions_tests.rs": "c34c2de6c22c14801abba9ee25d169c3d542f0785a4b32406c5ff16e44c85aa9",
    "crates\\core\\src\\kernel\\builder.rs": "57471d65c8058adbe23623143b336db86e4e1eafbf059eda6211cbfebccd6d56",
    "crates\\core\\src\\kernel\\mod.rs": "b9f43e5da3d22e43d601423d81f1338ead54c5f0a538e16965c23733ecb5a889",
    "crates\\core\\src\\kernel\\state.rs": "263ed5433755ef94ea8507543ca5cbccf454d345c58228aba9776ca9b424f079",
    "crates\\core\\src\\kernel\\turn.rs": "59a6e14719fb942993a42b438f22a155d2027e309b64849b69f99874f8204a2c",
    "crates\\core\\src\\lib.rs": "025a6825f6ed42629856d1cb0e5e07f43d384e32923b10962eb68ca7c382dcdb",
    "crates\\core\\src\\long_project.rs": "edd3a8c85071e1340b4769d00c67f564ff8ca025d0ff68f86cf027102e11de30",
    "crates\\core\\src\\loop_hygiene.rs": "80d608def62e644fef5183c046d5c8804b96b656100733cf3be1cef1b06e5ea1",
    "crates\\core\\src\\loop_runtime\\mod.rs": "dd6a8627b7f54bb866bc4ffb232e3eefb1e9508018bbfc131187587cbba3b53a",
    "crates\\core\\src\\loop_runtime\\model_step.rs": "e01520bc3ae23a591c554c251c5f936e694e3eafbd84ccedb187caa34659c3a1",
    "crates\\core\\src\\loop_runtime\\provider_step.rs": "046bc9cb04de96b4779394c2f17e83873561cbdf051fd7d52f9132bbba8e3e68",
    "crates\\core\\src\\loop_runtime\\retry_integration_tests.rs": "33b153fd0356bcdd795eb68f5d87bca1d8e8c5ff37551bb0841ca0694c71359f",
    "crates\\core\\src\\loop_runtime\\tool_step.rs": "74dc9789b19b5c589c843831770d9006e40f9d4d4d8832e36a2e567f698a609d",
    "crates\\core\\src\\runtime_core.rs": "714066711d6b723baceb6ddf80f5e0f9a5cc98220be3a4f469a10ef55a556452",
    "crates\\core\\src\\scheduler.rs": "d9d97a698b905d860a1628df05167d567af19185767006d4c10a1b74a5d90830",
    "crates\\core\\src\\subagent.rs": "de2bf3dc4c1dd67ddf3b0e05376553815bbbceb0cb0961e9c1279f5e47629b38",
    "crates\\core\\src\\subagent_tests.rs": "577103ac2a5d3f07c7aca686eabd883e69797544d44c04e39bd4f40f1ba6b1b8",
    "crates\\core\\src\\supervisor.rs": "db85fd5871237c8416fcb4abc7e325af43ab49d96bf30b47b262e081f4a145ca",
    "crates\\core\\src\\task_queue.rs": "a8fe4c2692aa6d6c03b02a035125e7206f79ce31de9c78c71dd356c43bdf8eb3",
    "crates\\core\\src\\task_queue_tests.rs": "beb11fa02ce4f211ec3f95e13ccdf912ae9385c1c1d2a723697c689bc62e1964",
    "crates\\core\\src\\tests.rs": "5121556eb1c07ce50515ec2f929ee7f6887216078fdcfa30b81911dd574b420c",
    "crates\\core\\src\\token.rs": "ae32239876684a5cf635634e2082520adc930c7aadf5ccad0f20f32774bb71cc",
    "crates\\core\\src\\user_input.rs": "9c43ec080df18e163db805aafb26985e8e4e8f0ccc100254c079a0113a110083",
    "crates\\core\\src\\user_input_tests.rs": "ffa948f251db087e4afb25ddc5ac528af95cf4d9aaa4f019b5314338dc1e342e",
    "crates\\evolution\\Cargo.toml": "1bcd638b5aec1a2798ed53faf3e375726a69878f63d8b3f0191cfcd8f5a649c7",
    "crates\\evolution\\src\\config.rs": "4d8c02f1ae2f35ac95dd477e521928c813911348dbc4dbebe9e98f6f9f7a51f6",
    "crates\\evolution\\src\\engine.rs": "9042950c27bf5255d2fc4ba83585e5dc11f05c8da5525778c6ec666a516e705e",
    "crates\\evolution\\src\\lib.rs": "7fbb611dd8865b6efdc5da9fd2dbfcc367aac607879ec0e0540c1ec175bbe363",
    "crates\\evolution\\src\\policy.rs": "420d1aad95a52dc1fbee68047cd2aeecb572e7c1371e48ac5364f715d3242472",
    "crates\\evolution\\src\\storage.rs": "2b6b7932336515bb06e22276c3a620159b3bc6e3171929ddf199678c2f09b50e",
    "crates\\evolution\\src\\types.rs": "239982bcbba0fb171978e75c1ac0cf1b479b2620176dc55a28c74b132d3070f3",
    "crates\\evolution\\src\\worker.rs": "2410c6a6936233fae2af924832b57b283afc6ef4466bd51c9b2f12f9f54411d9",
    "crates\\lexical\\Cargo.toml": "80acf5ea937c2077504119c2a86e2ffc8071131725a21ffc0e9cd4a87e33718a",
    "crates\\lexical\\src\\lib.rs": "bcd7bd04349afd87b38c5f19b3c13da43a80b615b018c1536d135a7ea43c6dbc",
    "crates\\mcp\\Cargo.toml": "c80d9052af07042351ab295d4c9a102d7fcec4f47c55e3969e759cdb607c2e95",
    "crates\\mcp\\src\\bridge.rs": "9439b1297a0c2228f18e492f0203ecdd6006d59756d5053f4151441ef1bf79f7",
    "crates\\mcp\\src\\client.rs": "e6192ab80f5a5dcfea438aa701da9c10b8da5b452bd36ad2b1f7adfd09dd549a",
    "crates\\mcp\\src\\config.rs": "ff1d7d8f3cb9a3bb4fd5e881587068da80560f1c3c74ac6017ee1c01ba3ae8d7",
    "crates\\mcp\\src\\gateway.rs": "76229f9e3a690b4f2e16e8a5784c45b0d461063ba38e39aecf77c4997170486e",
    "crates\\mcp\\src\\lib.rs": "636298a16bd61185ba6eccd65baa13920cbb2c613b5cb2675edca8fa12614074",
    "crates\\mcp\\src\\manager.rs": "9c04c41aa0ee523d000ab33a66f806067d798e060e2158b81f43f9e06be4d905",
    "crates\\mcp\\src\\transport\\http.rs": "9f809ceaea936db475913464addaeb2abe2e06b0d5fe040519f15d3a1aaaf7e7",
    "crates\\mcp\\src\\transport\\mod.rs": "74a942494da7949ce7a8a256c37b8459c70e89a7043033eac5a3910e0100a04c",
    "crates\\mcp\\src\\transport\\stdio.rs": "b7e8db1ae7e07d8768e3cd48fd3e142328f617bebf995a049ad0fdcca63469a3",
    "crates\\mcp\\src\\transport\\websocket.rs": "6052f7b951b64ff1c9b704875342a2887ab705bb3e38a3299ae8ef75133c83e7",
    "crates\\mcp\\tests\\runtime_sandbox.rs": "b037ed5f954273fbffe798bea1562db297d18110e2b7f56105664a0ce591643f",
    "crates\\memory\\Cargo.toml": "687805f0845bfe3ef83a7b609cae6d22227f79f5e4693a57f138ed6b762eeab2",
    "crates\\memory\\src\\backup.rs": "1ec1db39d3dfd6e81a56922796113bde29f9734e0e21a7b40851e74d04f07a13",
    "crates\\memory\\src\\context.rs": "e534417075e1ad8168e3b4d2dda3576617501c7246909f6405161903faa2c285",
    "crates\\memory\\src\\error.rs": "bd50d62b69a248f5b313b6f89418171a59a408bb09cd26a14eeb191a9bac5db9",
    "crates\\memory\\src\\events.rs": "9b9711b0ac49123ea0110f89bf6b6346734b3616c9ab7a1907114085b22002bd",
    "crates\\memory\\src\\lib.rs": "c1d8cbbacf39c8dedb2668f90e372483a299ed04c026152159ae0da3925fed2c",
    "crates\\memory\\src\\long_term.rs": "aca9b1ec1a84a156ea6eca3bac552fc8b2ac0f7dad812c5a5695c6c269603ab2",
    "crates\\memory\\src\\message.rs": "c41ac7876b5f852daa3ea6e91dbab3e6ca2ee284ae54a808a60016469a6c83a3",
    "crates\\memory\\src\\migrations.rs": "6368cfdf45991fe6df90f809ca01594485fd6d3ad8bb9cd983010ffd5cef79a0",
    "crates\\memory\\src\\schema.rs": "111032362a34f7e45531e220951658fc5e27fe769ac80e0cb9010e29d314f344",
    "crates\\memory\\src\\scoped.rs": "8277b328321ee3f8afc67cf564672cf05d5d47945287333f4ce084a125d09260",
    "crates\\memory\\src\\session.rs": "25da7ea06fbe2bb7be0cf30da026cfcba9245ba1046df3e623cc567f1083c6b4",
    "crates\\memory\\src\\store.rs": "675b171433ebc66c8cb2dda68458cdbb0c04c62a66de6be97f397638f7bc67e0",
    "crates\\memory\\src\\tests.rs": "f29c39069bfa4dcf02d7e8792788eebd71e4caf103d604810d8ad3af4f0cefab",
    "crates\\memory\\src\\types.rs": "f28f1f890dba68a58a5f94e308155eccd7d679304d3a4b1d04e13fdffae3a25a",
    "crates\\model\\Cargo.toml": "ac3455a713665778be51d7bbb139fe9b99e5ae0019d22e3ddf4f9769a9b48a3b",
    "crates\\model\\src\\adapter.rs": "3125dc4db081e00257177054c8dc0058b70d28598a428696cc8d3f51607fd84d",
    "crates\\model\\src\\auth.rs": "a0cd8132f47a6fb8884bc931e47026999229f1c03ef6f7013f9614df8144c1d7",
    "crates\\model\\src\\codex_browser.rs": "d0ed98f58dc3ae5d3058106651839b1f8a15fe88ab9773d5a4b4e7490f9ed468",
    "crates\\model\\src\\codex_device.rs": "db04e8145454ac944c415c3208f2ee3c4d1a42451235a6feada40ca9a35ab7a6",
    "crates\\model\\src\\hedge.rs": "058ddbc25c09670b0f3bd4388eca723c8de6a2145282f4bd1408be27bec2581c",
    "crates\\model\\src\\lib.rs": "250dcedeb13f855759a9529b834dd1b63280b5ecae9063414c57a17cbca9384d",
    "crates\\model\\src\\native\\anthropic.rs": "c16ce457a47742962097724d9abcbf6697f8f2a8eb2360b5bc60eb084d3ac158",
    "crates\\model\\src\\native\\bedrock.rs": "0cfc7fb68cf88101637a3ba93f01b644cbd039f90369d989a11c8f6633b65bc7",
    "crates\\model\\src\\native\\google.rs": "77e0329fe2fa8ec85c478c7a1b138a40d53d1dd47d832fa71f67e992f483c492",
    "crates\\model\\src\\native\\mod.rs": "1fe35d648e91c349a22de0269d1b161184140bf6d663f1474b29d88cc116e968",
    "crates\\model\\src\\native\\radius.rs": "3a4641d207d416732623e5af3d27a07a3fac7004d37fcba7a8c6147e8f6434af",
    "crates\\model\\src\\native\\transport.rs": "c55301230edc2aa431b8440f95c263e429131319fbe05686f2958cfa92df0d2a",
    "crates\\model\\src\\openai.rs": "abd1e63f1c154270a0e50935aaeda93b213b83d63a74aced7cf6fa97de82137a",
    "crates\\model\\src\\openai_compatible.rs": "de34ca5011c28538dfd405a764769a1da172ccc9a9177f5fe25dc5b4746b1773",
    "crates\\model\\src\\providers.rs": "7d141995c186ba944e46d44bf3160cee3c68f41342bf147d69945e8f395fab17",
    "crates\\model\\src\\registry.rs": "b5d720d9743fa4c05d0da754af7156289cba3f92a612f449fb13ae4503a2fa4a",
    "crates\\model\\src\\retry.rs": "864ef8c02b7bf6665959f2d1b93b56c7b10f0cfa137d41ad9ecececf2f3409b3",
    "crates\\model\\src\\search\\compatible.rs": "ad708830ee186f25a3d8baf3cb7b2f05882ffec2d12af095ae96b3155a82032e",
    "crates\\model\\src\\search.rs": "e58d83ee398697ba9934287953dc9c58ad30d304ead6f9405e1bd71f0c4a6c46",
    "crates\\model\\src\\stats.rs": "9df980e5a60b2552ac43ceecf886b08b73711adf824589d87b9bf57411b155f0",
    "crates\\model\\src\\workbuddy.rs": "098ae98d6aa651c764187b7b0a72ced9bcad0e383f260ab8ab9509c4be5d3eb0",
    "crates\\sandbox\\Cargo.toml": "ec7f9e8ab2186667163807b856f9bf6678d76beecbd96b9ac169d9f6729e8418",
    "crates\\sandbox\\src\\broker.rs": "f5c11712260cb4fbabdaf49f1237a9ec053fd55992e7158e882a1b20ca8992b4",
    "crates\\sandbox\\src\\lib.rs": "f9c5c87ae35ba75f2ba3d0d90429637655bd75c4d2e4eca50304dad319d2534c",
    "crates\\sandbox\\src\\linux.rs": "39a827b86f13df531105c922181618d7df27fb076f3667ea0a032c4022b7fff3",
    "crates\\sandbox\\tests\\linux_escape_suite.rs": "be8c4481dbd263305c438e81f80b3f89e57ae1d7e154a849dd8cfdf68f36168c",
    "crates\\scoped\\Cargo.toml": "b33389d31cd5b09e5fc5a39c789878711f81ece95ba850f18dd428d367f4b077",
    "crates\\scoped\\src\\lib.rs": "26f6c31d140e8c41849251b1cb76dc3527cba9b4c4c218071914d9723c92c6d6",
    "crates\\skill\\Cargo.toml": "1aff5662f243b4060561bfe10ffc11675c33c7f4f48bc90598e193ce35aa2350",
    "crates\\skill\\src\\lib.rs": "fccf67b32ccfd4128df1feaa9d02b92f01e37d0f9b23993cdc71e35a914bf25f",
    "crates\\skill\\tests\\agent_skills.rs": "beed7572d7b8d3b0b24bd210234ab4a437395dc37e1121a17300e0e21b1a802f",
    "crates\\tool\\Cargo.toml": "7111a93c8efe95733e77da2091137d7adb58a058f9c11a0dbf6872fc6f4bf9f0",
    "crates\\tool\\examples\\web_latency.rs": "df982c03b60f3161b0764ba4e3c94647bc2c36e28c25e1f3f52b34ded1c7d59e",
    "crates\\tool\\src\\browser.rs": "6b99badf2e0611c3f6c1130c1600b0b72cd235e42ec9dfe9e1938646c833b8b1",
    "crates\\tool\\src\\desktop.rs": "4c47a29841ef12be545250a0de422f25444c752e22dbe1426a13b1cd3d41f402",
    "crates\\tool\\src\\desktop_unix.rs": "4919f38bbdfc456f76d80d8994a1efb1e8260d33fb746bbff66a74e731b8af8d",
    "crates\\tool\\src\\desktop_windows.rs": "f5cafa8e8b03d3971dff4c767e9420eee964b5212c3448e99b57f899ec990ba2",
    "crates\\tool\\src\\discovery.rs": "aba1efd997770a34caeff9d109dbbfb90bf7eb01d658ad5b274520629d5fac83",
    "crates\\tool\\src\\environment.rs": "f2d51594dd06d4f1b27ded0cdb5927e8a3f69cdbf2ebd51b8b75edd7372ed8f9",
    "crates\\tool\\src\\filesystem.rs": "211700d4decc98e441180ea4c8853f0b1480b6b0b087f74a3c3919d15cf61525",
    "crates\\tool\\src\\find.rs": "754b6db8b5ac2c7eec4e3602cabda59f13d71d70059858f6c08b7bef7b528456",
    "crates\\tool\\src\\host_access.rs": "860c69981e74c77080b6fe62185db23d356edc3a11f7c1e2d55b2ec88346e284",
    "crates\\tool\\src\\lib.rs": "ea5bc29b300ff60c14e4986310e7bc925043b5233392b9e59164be30bc08bc19",
    "crates\\tool\\src\\patch.rs": "a736c66737d4b767ed7aabe7f254531d854b0c0ddee19f17c07968a4fb43752b",
    "crates\\tool\\src\\permission.rs": "cdfb9316e0c6abc5898cf5ed5df2b14c10884e7dd46d24bf20e81935c04ca590",
    "crates\\tool\\src\\resources.rs": "f3119b5f70edca792b0c9dd671da248ff2f656315bb4f837f821e321f2f0d0ac",
    "crates\\tool\\src\\result.rs": "d7d203a66457362449011768d1aaad69d9341fe0fbf4139952bd03dca2829bcc",
    "crates\\tool\\src\\sandboxed.rs": "264d078a6d99f778b20b1fd8baad6fcc949ad671d13b4cea747ddcdc0c7af97d",
    "crates\\tool\\src\\search.rs": "5ad4374a26bbe16271b49c7541243c9b5bded2b2ed16f76f1c5e361b30b67340",
    "crates\\tool\\src\\shell.rs": "4bb262bd520c7419cf94ba1f03401ed19630d36e3c852840537a599df129c082",
    "crates\\tool\\src\\ssh.rs": "f855716a32496359442b8c5767d594862232b354698616d7aed6db342e696100",
    "crates\\tool\\src\\task_source.rs": "eea6462c02c07293e969067c1c416adc3bac0e000d1a903dc94268bb5574f4e7",
    "crates\\tool\\src\\telemetry.rs": "694f5bcff8f99e69c99a9fb3110b32f65765aaea5484296d6d916ccf3b0b7c16",
    "crates\\tool\\src\\view_image.rs": "7a961d255f1904e7cea7b8f75fc67c20520b079d9d882b0831cae79c4ca36a56",
    "crates\\tool\\src\\web\\native.rs": "11cf35b24ff59d1ba94ad31e6d6684bfba08eee8495c279f1a095fddd4de6826",
    "crates\\tool\\src\\web\\network.rs": "639716ceff0072dfe3e1543453a43cb2710457af40cb5f1fd0afedb64c927178",
    "crates\\tool\\src\\web\\providers.rs": "6c9140718fbb65ba23c944a78822723dfd534181b468c4ed04f8484c0f5677db",
    "crates\\tool\\src\\web\\remote.rs": "9b4e4b4ee1fe24531ffb512be3a791ff2a1b69babb3b7ee84b9a4845d0cb92d4",
    "crates\\tool\\src\\web\\router.rs": "7b726bd2b2b14868b724498d4a2d7917057ed527c95fabe4b92c8da55fefae03",
    "crates\\tool\\src\\web.rs": "ebaf65815372388d67e2bc344c8ee73610d9e4ddbbbafbb0a16814b901d19b9a",
    "crates\\tool\\src\\workspace.rs": "9dceab45ff72ab6998e61c2db8b37eecedf9226e368dd00a8735d3b1eec4e34f",
    "crates\\tool\\tests\\discovery.rs": "f452339edfaa7bca86aa447b3f37e8268864341139baf841d92e2d276ca892e5",
    "crates\\tool\\tests\\resources.rs": "5861da178aa85d48f983fd249dc998b504cfee3555c0167273b3a6f0e1c25730",
    "crates\\tool\\tests\\verbatim.rs": "80ddfc3c75bf879ac1fef880bbb7c124f504891febc36657ec8a1cd1f310a4f0"
  }
}
```

Latency excludes fixture setup and warmup. Resources cover the whole worker (including setup); CPU is % of one logical core and can exceed 100%. RSS is process working set, not Rust heap. Null telemetry means unavailable, never zero.

Fresh-process CLI startup does not flush OS file caches. CLI --version/--help only measures that command, not a fully initialized agent. Startup wall latency includes parent launch/wait; a separate thread samples resources every 2 ms. Kernel init excludes process/Tokio initialization.

All mock-labelled results include deterministic boundary costs; direct mock baselines are reported separately and are not subtracted. No model/API, external tool, network, Git worktree or durable child-host execution is measured.

P95 uses nearest rank over batch-normalized ns/op, not individual operation tail latency. Throughput = total operations / total timed duration. Three or more independent processes per case; each round has its own fixture directory. No statistical claim from a single host or CI threshold.

## Results

Case; size; concurrency; mean / median / P95 / min / max (µs/op); ops/s; maximum worker peak RSS (MiB); boundary

- `session_load`; 16; 1; 379.513 / 369.650 / 433.600 / 338.700 / 532.000; 2635.0; 6.76; local production path
- `session_load`; 128; 1; 2371.072 / 2342.900 / 2640.600 / 2126.800 / 3048.500; 421.8; 7.09; local production path
- `session_load`; 1024; 1; 18072.080 / 17872.900 / 19609.800 / 16630.100 / 19997.000; 55.3; 8.72; local production path
- `session_resume_local`; 16; 1; 376.213 / 360.700 / 511.900 / 323.200 / 643.700; 2658.1; 6.71; local production path
- `session_resume_local`; 128; 1; 2365.578 / 2338.200 / 2618.000 / 2121.900 / 2752.100; 422.7; 7.27; local production path
- `session_resume_local`; 1024; 1; 2819.507 / 2761.350 / 3182.100 / 2490.400 / 4010.100; 354.7; 8.99; local production path
- `session_snapshot_load`; 16; 1; 79.105 / 65.250 / 139.500 / 59.900 / 244.600; 12641.4; 6.71; local production path
- `session_snapshot_load`; 128; 1; 219.507 / 214.050 / 264.700 / 171.800 / 420.500; 4555.7; 7.15; local production path
- `session_snapshot_load`; 1024; 1; 1203.402 / 1205.050 / 1337.400 / 1032.000 / 1378.000; 831.0; 9.56; local production path
- `context_select`; 16; 1; 3.421 / 3.280 / 3.825 / 3.051 / 7.033; 292323.6; 6.65; local production path
- `context_select`; 128; 1; 25.215 / 24.576 / 29.777 / 20.581 / 35.381; 39659.7; 6.88; local production path
- `context_select`; 1024; 1; 115.423 / 101.460 / 190.763 / 74.177 / 205.148; 8663.8; 8.54; local production path
- `context_serialize`; 16; 1; 1.932 / 1.910 / 2.145 / 1.813 / 2.266; 517473.4; 6.67; local production path
- `context_serialize`; 128; 1; 14.491 / 14.460 / 16.165 / 12.374 / 17.090; 69009.9; 6.96; local production path
- `context_serialize`; 1024; 1; 128.063 / 125.335 / 163.995 / 105.754 / 167.182; 7808.6; 8.15; local production path
- `context_tokens`; 16; 1; 1.431 / 1.403 / 1.574 / 1.273 / 1.995; 698812.0; 6.57; local production path
- `context_tokens`; 128; 1; 11.294 / 11.259 / 12.195 / 10.577 / 12.463; 88545.3; 6.66; local production path
- `context_tokens`; 1024; 1; 87.420 / 87.390 / 92.023 / 82.242 / 93.256; 11439.0; 7.51; local production path
- `memory_query`; 16; 1; 227.390 / 217.050 / 285.500 / 196.100 / 371.200; 4397.7; 8.35; local production path
- `memory_query`; 128; 1; 1535.055 / 1527.950 / 1629.200 / 1423.700 / 1688.300; 651.4; 17.17; local production path
- `memory_query`; 1024; 1; 20574.378 / 20536.150 / 21449.300 / 19453.100 / 22172.900; 48.6; 52.23; local production path
- `memory_recall`; 16; 1; 261.317 / 257.700 / 283.300 / 239.800 / 394.100; 3826.8; 8.37; local production path
- `memory_recall`; 128; 1; 1861.162 / 1851.450 / 2030.400 / 1670.000 / 2069.100; 537.3; 17.27; local production path
- `memory_recall`; 1024; 1; 22873.273 / 22882.700 / 23935.100 / 21699.100 / 25047.600; 43.7; 52.62; local production path
- `memory_store`; 16; 1; 4448.373 / 4274.400 / 6891.600 / 3474.300 / 7098.000; 224.8; 7.55; local production path
- `memory_store`; 128; 1; 4376.967 / 4262.500 / 6156.900 / 3483.400 / 7161.900; 228.5; 11.77; local production path
- `memory_store`; 1024; 1; 4667.613 / 4088.400 / 6809.000 / 3390.800 / 15050.500; 214.2; 26.03; local production path
- `tool_register`; 16; 1; 0.362 / 0.328 / 0.444 / 0.269 / 1.569; 2761795.2; 6.53; runtime + deterministic mocks
- `tool_register`; 128; 1; 0.334 / 0.330 / 0.430 / 0.263 / 0.527; 2996137.8; 6.55; runtime + deterministic mocks
- `tool_register`; 1024; 1; 0.304 / 0.300 / 0.329 / 0.280 / 0.372; 3288181.5; 7.65; runtime + deterministic mocks
- `tool_lookup`; 16; 1; 0.179 / 0.175 / 0.201 / 0.161 / 0.272; 5578281.9; 6.57; runtime + deterministic mocks
- `tool_lookup`; 128; 1; 0.186 / 0.185 / 0.202 / 0.173 / 0.211; 5362888.8; 6.64; runtime + deterministic mocks
- `tool_lookup`; 1024; 1; 0.253 / 0.239 / 0.339 / 0.198 / 0.490; 3949707.1; 7.50; runtime + deterministic mocks
- `tool_dispatch`; 16; 1; 75.354 / 74.394 / 83.381 / 65.781 / 97.719; 13270.8; 8.74; runtime + deterministic mocks
- `tool_dispatch`; 128; 1; 39.269 / 38.808 / 41.334 / 36.661 / 49.342; 25465.7; 10.44; runtime + deterministic mocks
- `tool_dispatch`; 1024; 1; 608.858 / 607.682 / 625.335 / 597.386 / 648.594; 1642.4; 21.90; runtime + deterministic mocks
- `kernel_init`; 1; 1; 2.343 / 1.600 / 2.900 / 1.200 / 40.100; 426742.5; 6.56; runtime + deterministic mocks
- `session_create`; 1; 1; 2354.980 / 2282.100 / 3277.000 / 1769.200 / 4663.400; 424.6; 6.59; local production path
- `mock_tool`; 1; 1; 0.185 / 0.200 / 0.300 / 0.100 / 0.700; 5405405.4; 6.57; mock-boundary baseline
- `mock_model`; 1; 1; 0.313 / 0.300 / 0.800 / 0.200 / 1.000; 3191489.4; 6.59; mock-boundary baseline
- `kernel_turn`; 1; 1; 496.912 / 474.200 / 611.100 / 395.600 / 713.200; 2012.4; 7.96; runtime + deterministic mocks
- `subagent_lifecycle`; 1; 1; 348.178 / 335.350 / 456.300 / 273.700 / 679.200; 2872.1; 8.09; runtime + deterministic mocks
- `concurrency`; 16; 1; 513.874 / 518.709 / 552.075 / 455.206 / 575.819; 1946.0; 8.27; runtime + deterministic mocks
- `concurrency`; 16; 4; 207.680 / 209.481 / 232.106 / 164.756 / 252.006; 4815.1; 10.18; runtime + deterministic mocks
- `concurrency`; 16; 8; 190.457 / 187.575 / 220.000 / 152.156 / 247.625; 5250.5; 11.96; runtime + deterministic mocks
- `concurrency`; 64; 1; 459.236 / 460.230 / 483.564 / 424.062 / 493.305; 2177.5; 8.46; runtime + deterministic mocks
- `concurrency`; 64; 4; 166.321 / 166.376 / 181.356 / 146.231 / 188.664; 6012.5; 10.64; runtime + deterministic mocks
- `concurrency`; 64; 8; 148.761 / 148.088 / 161.109 / 129.469 / 168.755; 6722.2; 12.36; runtime + deterministic mocks
- `startup_version`; 1; 1; 12813.972 / 12520.250 / 14246.400 / 11443.400 / 15019.200; 78.0; 11.61; local production path
- `startup_help`; 1; 1; 13078.237 / 12857.400 / 14749.000 / 11761.600 / 16787.200; 76.5; 11.75; local production path

## Independent rounds

Each line: case / size / concurrency / round; mean µs/op; worker CPU % of one core; sampled max RSS MiB. Whole-worker telemetry cannot be attributed solely to timed operations.

- session_load / 16 / 1 / 1: 375.200; CPU 0.0; RSS 6.5859375
- session_load / 128 / 1 / 1: 2481.865; CPU 64.51098813080392; RSS 6.9921875
- session_load / 1024 / 1 / 1: 18668.710; CPU 92.84635543701759; RSS 8.70703125
- session_resume_local / 16 / 1 / 1: 421.005; CPU 19.97045012435999; RSS 6.609375
- session_resume_local / 128 / 1 / 1: 2360.715; CPU 72.48617930181311; RSS 7.21484375
- session_resume_local / 1024 / 1 / 1: 3022.020; CPU 67.48895205854801; RSS 8.50390625
- session_snapshot_load / 16 / 1 / 1: 88.870; CPU 50.877535736381105; RSS 6.57421875
- session_snapshot_load / 128 / 1 / 1: 216.630; CPU 0.0; RSS 7.0390625
- session_snapshot_load / 1024 / 1 / 1: 1246.205; CPU 28.629407210946052; RSS 9.109375
- context_select / 16 / 1 / 1: 3.337; CPU 22.54553115107021; RSS 6.4453125
- context_select / 128 / 1 / 1: 26.041; CPU 69.87402551687613; RSS 6.76171875
- context_select / 1024 / 1 / 1: 126.673; CPU 93.40484109071747; RSS 8.48046875
- context_serialize / 16 / 1 / 1: 1.947; CPU 52.25569001757464; RSS 6.546875
- context_serialize / 128 / 1 / 1: 14.892; CPU 74.44594351002102; RSS 6.89453125
- context_serialize / 1024 / 1 / 1: 134.679; CPU 96.69254297034598; RSS 8.05078125
- context_tokens / 16 / 1 / 1: 1.488; CPU 27.01926884561579; RSS 6.390625
- context_tokens / 128 / 1 / 1: 11.098; CPU 74.86213389422039; RSS 6.55859375
- context_tokens / 1024 / 1 / 1: 88.871; CPU 90.92091119758365; RSS 7.43359375
- memory_query / 16 / 1 / 1: 236.465; CPU 12.031512166650112; RSS 8.08984375
- memory_query / 128 / 1 / 1: 1544.170; CPU 14.118583450735406; RSS 17.10546875
- memory_query / 1024 / 1 / 1: 20478.420; CPU 18.113088645932617; RSS 52.12890625
- memory_recall / 16 / 1 / 1: 265.235; CPU 26.224054968975892; RSS 8.13671875
- memory_recall / 128 / 1 / 1: 1838.110; CPU 16.073784660223335; RSS 17.265625
- memory_recall / 1024 / 1 / 1: 22620.340; CPU 21.18345185647253; RSS 52.48828125
- memory_store / 16 / 1 / 1: 4623.335; CPU 26.516027347569963; RSS 7.3515625
- memory_store / 128 / 1 / 1: 4409.325; CPU 10.626189623180695; RSS 11.765625
- memory_store / 1024 / 1 / 1: 4722.320; CPU 11.10851081928179; RSS 26.00390625
- tool_register / 16 / 1 / 1: 0.321; CPU 0.0; RSS 6.37109375
- tool_register / 128 / 1 / 1: 0.358; CPU 0.0; RSS 6.45703125
- tool_register / 1024 / 1 / 1: 0.304; CPU 84.96619478330221; RSS 7.609375
- tool_lookup / 16 / 1 / 1: 0.184; CPU 27.14851147969378; RSS 6.34765625
- tool_lookup / 128 / 1 / 1: 0.185; CPU 90.73672412841933; RSS 6.546875
- tool_lookup / 1024 / 1 / 1: 0.236; CPU 87.46791059618786; RSS 7.48828125
- tool_dispatch / 16 / 1 / 1: 72.822; CPU 11.553252545597452; RSS 8.609375
- tool_dispatch / 128 / 1 / 1: 40.155; CPU 24.555297665997347; RSS 10.1796875
- tool_dispatch / 1024 / 1 / 1: 609.733; CPU 96.49572530002919; RSS 21.140625
- kernel_init / 1 / 1 / 1: 1.560; CPU 65.15045063263693; RSS 6.3515625
- session_create / 1 / 1 / 1: 2235.085; CPU 20.86000632809152; RSS 6.36328125
- mock_tool / 1 / 1 / 1: 0.195; CPU 31.348245401087006; RSS 6.3671875
- mock_model / 1 / 1 / 1: 0.250; CPU 41.996591892574735; RSS 6.34375
- kernel_turn / 1 / 1 / 1: 517.135; CPU 0.0; RSS 7.6171875
- subagent_lifecycle / 1 / 1 / 1: 359.145; CPU 5.802275531776232; RSS 8.01171875
- concurrency / 16 / 1 / 1: 510.576; CPU 33.63933918882097; RSS 8.1875
- concurrency / 16 / 4 / 1: 194.572; CPU 46.12478044604507; RSS 9.92578125
- concurrency / 16 / 8 / 1: 185.541; CPU 56.1308294378894; RSS 11.640625
- concurrency / 64 / 1 / 1: 459.590; CPU 75.02144742179455; RSS 8.46484375
- concurrency / 64 / 4 / 1: 165.197; CPU 139.5341612328789; RSS 10.0625
- concurrency / 64 / 8 / 1: 147.532; CPU 143.5104523529325; RSS 12.27734375
- startup_version / 1 / 1 / 1: 12237.635; CPU 540.5779426900889; RSS 11.49609375
- startup_help / 1 / 1 / 1: 13355.110; CPU 710.8034543152404; RSS 11.70703125
- session_load / 128 / 1 / 2: 2355.915; CPU 60.46227618316452; RSS 7.00390625
- session_load / 1024 / 1 / 2: 17281.325; CPU 96.30870481568377; RSS 8.54296875
- session_resume_local / 16 / 1 / 2: 342.870; CPU 54.38641679661394; RSS 6.57421875
- session_resume_local / 128 / 1 / 2: 2219.140; CPU 65.90485574323384; RSS 7.08984375
- session_resume_local / 1024 / 1 / 2: 2699.770; CPU 64.97642482037051; RSS 8.49609375
- session_snapshot_load / 16 / 1 / 2: 66.955; CPU 0.0; RSS 6.52734375
- session_snapshot_load / 128 / 1 / 2: 205.405; CPU 27.76538824453444; RSS 7.01953125
- session_snapshot_load / 1024 / 1 / 2: 1170.685; CPU 62.6425117141497; RSS 9.390625
- context_select / 16 / 1 / 2: 3.304; CPU 117.18919924338904; RSS 6.46875
- context_select / 128 / 1 / 2: 25.752; CPU 75.00289725477394; RSS 6.8515625
- context_select / 1024 / 1 / 2: 123.015; CPU 93.52247949632849; RSS 8.3828125
- context_serialize / 16 / 1 / 2: 1.972; CPU 53.18008393093566; RSS 6.51171875
- context_serialize / 128 / 1 / 2: 13.837; CPU 84.62168914820485; RSS 6.875
- context_serialize / 1024 / 1 / 2: 126.362; CPU 97.29352756753727; RSS 8.02734375
- context_tokens / 16 / 1 / 2: 1.420; CPU 27.961553534966704; RSS 6.34375
- context_tokens / 128 / 1 / 2: 11.498; CPU 63.33038804808652; RSS 6.64453125
- context_tokens / 1024 / 1 / 2: 86.263; CPU 93.79172824244903; RSS 7.44921875
- memory_query / 16 / 1 / 2: 222.305; CPU 40.6583045725522; RSS 8.0234375
- memory_query / 128 / 1 / 2: 1504.425; CPU 9.443449902271626; RSS 17.0625
- memory_query / 1024 / 1 / 2: 20650.485; CPU 19.662779845926178; RSS 52.00390625
- memory_recall / 16 / 1 / 2: 256.300; CPU 13.63899819485131; RSS 8.15625
- memory_recall / 128 / 1 / 2: 1887.770; CPU 22.77760543735335; RSS 17.2578125
- memory_recall / 1024 / 1 / 2: 22920.345; CPU 23.66093359721773; RSS 52.6171875
- memory_store / 16 / 1 / 2: 4340.150; CPU 7.036332694922177; RSS 7.36328125
- memory_store / 128 / 1 / 2: 4355.845; CPU 14.803104165265307; RSS 11.6171875
- memory_store / 1024 / 1 / 2: 4693.500; CPU 12.808322194603303; RSS 26.03125
- tool_register / 16 / 1 / 2: 0.363; CPU 77.52745712421512; RSS 6.359375
- tool_register / 128 / 1 / 2: 0.348; CPU 0.0; RSS 6.48828125
- tool_register / 1024 / 1 / 2: 0.297; CPU 27.606885024612087; RSS 7.52734375
- tool_lookup / 16 / 1 / 2: 0.179; CPU 0.0; RSS 6.39453125
- tool_lookup / 128 / 1 / 2: 0.184; CPU 42.87723390388639; RSS 6.53515625
- tool_lookup / 1024 / 1 / 2: 0.241; CPU 79.77130887669598; RSS 7.4453125
- tool_dispatch / 16 / 1 / 2: 75.257; CPU 14.515677117087122; RSS 8.53125
- tool_dispatch / 128 / 1 / 2: 39.144; CPU 32.047125406377276; RSS 10.28515625
- tool_dispatch / 1024 / 1 / 2: 612.192; CPU 97.14770506110635; RSS 21.5859375
- kernel_init / 1 / 1 / 2: 3.895; CPU 38.791541085517665; RSS 6.359375
- session_create / 1 / 1 / 2: 2356.920; CPU 20.75081426195164; RSS 6.359375
- mock_tool / 1 / 1 / 2: 0.180; CPU 0.0; RSS 6.3515625
- mock_model / 1 / 1 / 2: 0.280; CPU 0.0; RSS 6.36328125
- kernel_turn / 1 / 1 / 2: 497.340; CPU 8.672337712578654; RSS 7.73828125
- subagent_lifecycle / 1 / 1 / 2: 347.370; CPU 9.229311624285996; RSS 7.97265625
- concurrency / 16 / 1 / 2: 530.822; CPU 36.25099540968572; RSS 8.24609375
- concurrency / 16 / 4 / 2: 212.462; CPU 56.87388375524149; RSS 9.8984375
- concurrency / 16 / 8 / 2: 202.023; CPU 46.683927598437236; RSS 10.9453125
- concurrency / 64 / 1 / 2: 458.540; CPU 72.09838180534426; RSS 8.4140625
- concurrency / 64 / 4 / 2: 165.188; CPU 132.74478105445888; RSS 10.625
- concurrency / 64 / 8 / 2: 150.291; CPU 136.73431883484793; RSS 12.296875
- startup_version / 1 / 1 / 2: 13673.445; CPU 462.8978143816796; RSS 11.5078125
- startup_help / 1 / 1 / 2: 13161.860; CPU 501.87097499478057; RSS 11.72265625
- session_load / 16 / 1 / 2: 394.960; CPU 20.767569363681677; RSS 6.4296875
- session_load / 1024 / 1 / 3: 18266.205; CPU 89.16931860183269; RSS 8.66796875
- session_resume_local / 16 / 1 / 3: 364.765; CPU 25.42035092286042; RSS 6.4453125
- session_resume_local / 128 / 1 / 3: 2516.880; CPU 89.55885050243027; RSS 7.14453125
- session_resume_local / 1024 / 1 / 3: 2736.730; CPU 54.27397572416246; RSS 8.359375
- session_snapshot_load / 16 / 1 / 3: 81.490; CPU 0.0; RSS 6.53515625
- session_snapshot_load / 128 / 1 / 3: 236.485; CPU 52.407737226767026; RSS 6.9453125
- session_snapshot_load / 1024 / 1 / 3: 1193.315; CPU 32.741197856368295; RSS 8.9921875
- context_select / 16 / 1 / 3: 3.621; CPU 97.16269387843937; RSS 6.65234375
- context_select / 128 / 1 / 3: 23.850; CPU 67.72508024067507; RSS 6.84765625
- context_select / 1024 / 1 / 3: 96.582; CPU 98.715119732796; RSS 8.4453125
- context_serialize / 16 / 1 / 3: 1.878; CPU 88.21022165934953; RSS 6.43359375
- context_serialize / 128 / 1 / 3: 14.743; CPU 83.10956544556922; RSS 6.81640625
- context_serialize / 1024 / 1 / 3: 123.148; CPU 98.66482029268565; RSS 8.06640625
- context_tokens / 16 / 1 / 3: 1.385; CPU 77.5427829381064; RSS 6.37890625
- context_tokens / 128 / 1 / 3: 11.285; CPU 46.63956348353511; RSS 6.52734375
- context_tokens / 1024 / 1 / 3: 87.127; CPU 89.36454232295192; RSS 7.453125
- memory_query / 16 / 1 / 3: 223.400; CPU 0.0; RSS 8.05859375
- memory_query / 128 / 1 / 3: 1556.570; CPU 6.322290758476623; RSS 17.09375
- memory_query / 1024 / 1 / 3: 20594.230; CPU 19.66997743334301; RSS 52.23046875
- memory_recall / 16 / 1 / 3: 262.415; CPU 41.30014167600601; RSS 8.28515625
- memory_recall / 128 / 1 / 3: 1857.605; CPU 16.22521232590996; RSS 17.15234375
- memory_recall / 1024 / 1 / 3: 23079.135; CPU 21.097078509847854; RSS 52.49609375
- memory_store / 16 / 1 / 3: 4381.635; CPU 14.027178162668585; RSS 7.3046875
- memory_store / 128 / 1 / 3: 4365.730; CPU 18.59580359657718; RSS 11.73828125
- memory_store / 1024 / 1 / 3: 4587.020; CPU 12.089638713811752; RSS 26.015625
- tool_register / 16 / 1 / 3: 0.402; CPU 0.0; RSS 6.30859375
- tool_register / 128 / 1 / 3: 0.295; CPU 40.435173036662086; RSS 6.51953125
- tool_register / 1024 / 1 / 3: 0.311; CPU 28.909166934943737; RSS 7.5234375
- tool_lookup / 16 / 1 / 3: 0.175; CPU 29.827980841368596; RSS 6.3515625
- tool_lookup / 128 / 1 / 3: 0.190; CPU 46.00357723816604; RSS 6.46875
- tool_lookup / 1024 / 1 / 3: 0.283; CPU 89.42458540658097; RSS 7.4296875
- tool_dispatch / 16 / 1 / 3: 77.981; CPU 12.942022884975277; RSS 8.578125
- tool_dispatch / 128 / 1 / 3: 38.507; CPU 23.071969631529203; RSS 10.234375
- tool_dispatch / 1024 / 1 / 3: 604.647; CPU 96.94432728766921; RSS 21.140625
- kernel_init / 1 / 1 / 3: 1.575; CPU 0.0; RSS 6.36328125
- session_create / 1 / 1 / 3: 2472.935; CPU 0.0; RSS 6.34375
- mock_tool / 1 / 1 / 3: 0.180; CPU 37.74391628459758; RSS 6.37109375
- mock_model / 1 / 1 / 3: 0.410; CPU 118.41217795045256; RSS 6.375
- kernel_turn / 1 / 1 / 3: 476.260; CPU 14.644574993345504; RSS 7.70703125
- subagent_lifecycle / 1 / 1 / 3: 338.020; CPU 17.85449019002891; RSS 7.9140625
- concurrency / 16 / 1 / 3: 500.223; CPU 44.32935362128262; RSS 8.1640625
- concurrency / 16 / 4 / 3: 216.007; CPU 62.622824232594866; RSS 9.6875
- concurrency / 16 / 8 / 3: 183.808; CPU 68.65002522252779; RSS 11.5
- concurrency / 64 / 1 / 3: 459.578; CPU 67.25298618910284; RSS 8.2578125
- concurrency / 64 / 4 / 3: 168.578; CPU 129.00505174400442; RSS 10.3359375
- concurrency / 64 / 8 / 3: 148.460; CPU 113.00994855754266; RSS 12.0390625
- startup_version / 1 / 1 / 3: 12530.835; CPU 1033.655833953527; RSS 11.59375
- startup_help / 1 / 1 / 3: 12717.740; CPU 132.84757175894435; RSS 11.72265625
- session_load / 16 / 1 / 3: 368.380; CPU 72.77133422651252; RSS 6.4765625
- session_load / 128 / 1 / 3: 2275.435; CPU 66.06128457806182; RSS 6.9375

## Uncovered / limitations

- Session resume measures effective-context lookup, latest 128-message page and public select_context on fixed model messages; excludes private CLI conversion, identity, full resume and UI.
- Context select/token estimate/serde use production APIs. Full model request assembly is covered only indirectly by mock kernel turns. No model-based compression, summarization or quality evaluation.
- Memory recall measures memory_index + retrieve_index; excludes private CLI injection, embeddings, remote recall and cold legacy index migration. Memory store is a fixed-key upsert; not growing ingestion.
- Subagent lifecycle uses actual prepare/spawn/wait/fork/run_child logic with a mock ChildHost and no-op checkpoints; no durable provisioning, Git isolation, network, process child or crash-recovery claim.
- Concurrency uses real AgentSupervisor and kernels with a yielding constant provider. It measures scheduling throughput, not useful agent work or model serving throughput. Tool dispatch includes two mock model calls and the real scheduler.
- Resources are worker-level, include fixture seeding and warmup; CPU timer resolution limits short runs. Linux /proc is sampled; macOS resource telemetry currently unavailable. No per-operation CPU allocation or leak/stress/soak claim.
- Filesystem caches, antivirus, CPU frequency and background load are uncontrolled; keep host/power mode, fixture version, lockfiles, build flags and parameters identical for comparisons.

## Failures

All scheduled workers completed and their embedded correctness assertions passed.
