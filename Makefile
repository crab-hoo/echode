# Propagate every recipe/pipeline failure, including multi-command recipes.
# Apple's GNU make 3.81 does not support .SHELLFLAGS; pass options in SHELL.
SHELL := /bin/bash -euo pipefail
.PHONY: spike-verification-failures

.PHONY: bundled-plugins app dist xcode sign-setup spike-app spike-playhead spike-arrange-frame spike-piano-frame spike-audio spike-audition spike-plugin-tracks spike-scale check check-format check-base check-product-contracts check-real-hal verify verify-plan check-verification doctor agent-context debug-report fmt setup setup-bridge known-bugs advisories bless spike-isolation spike-draw spike-history spike-input check-input spike-engine spike-engine-smoke spike-bridge spike-render check-bridge check-bridge-features check-bridge-asan check-bridge-tsan check-tsan-runtime check-bridge-miri check-shell check-swift check-contract-controls contract-prerequisites verify-full rtsan

bundled-plugins:
	bash scripts/build-bundled-plugins.sh

app:
	bash scripts/build-app.sh

dist:
	bash scripts/build-dist.sh

xcode:
	bash scripts/generate-xcode.sh

sign-setup:
	python3 scripts/distribution.py setup-identity

# Product arrange and piano rendering continue while MainActor is deliberately busy.
spike-app:
	bash scripts/build-app.sh
	swift run --package-path swift/EchodeApp --scratch-path target/echode-app EchodeAppProbe "$(CURDIR)/target/spike-app.json"

# Real-window checks run under AppKit; the normal suite never invokes them.
spike-playhead:
	cargo build -q --locked --offline -p echode-ffi
	set -eu; header="$$(find target/debug/build -path '*echode-ffi-*/out/echode.h' -print0 | xargs -0 ls -t | head -1)"; \
	test -n "$$header"; \
	mkdir -p swift/EchodeBridge/CEchodeFfi/include "$(CURDIR)/target"; \
	cmp -s "$$header" swift/EchodeBridge/CEchodeFfi/include/echode.h || cp "$$header" swift/EchodeBridge/CEchodeFfi/include/echode.h; \
	frameworks="$$(xcrun --show-sdk-platform-path)/Developer/Library/Frameworks"; \
	test -d "$$frameworks/Testing.framework"; \
	swift build -q --package-path swift/EchodeApp --scratch-path target/echode-app-tests \
		-Xswiftc -warnings-as-errors --product EchodePlayheadProbe; \
	bin="$$(swift build --package-path swift/EchodeApp --scratch-path target/echode-app-tests --show-bin-path)"; \
	DYLD_FRAMEWORK_PATH="$$frameworks" \
	ECHODE_SPIKE_PLAYHEAD_REPORT="$(CURDIR)/target/spike-playhead.json" \
	"$$bin/EchodePlayheadProbe"

# Actual GPU pixels, without creating windows or controlling the computer UI.
spike-arrange-frame:
	cargo build -q -p echode-ffi
	swift run --package-path swift/EchodeApp --scratch-path target/echode-app EchodeAppProbe --offscreen "$(CURDIR)/target/spike-arrange-frame.json"

spike-piano-frame:
	cargo build -q -p echode-ffi
	swift run --package-path swift/EchodeApp --scratch-path target/echode-app EchodeAppProbe --offscreen-piano "$(CURDIR)/target/spike-piano-frame.json"

# Portable checks include required product regressions, once each.
check:
	$(MAKE) check-format
	$(MAKE) check-verification
	$(MAKE) check-base
	$(MAKE) check-swift
	$(MAKE) check-product-contracts

.PHONY: swift-actions check-swift-actions
swift-actions:
	cargo run -q --locked --offline -p echode-actions --example swift_actions -- --write swift/EchodeBridge/Sources/EchodeBridge/EditAction.swift

check-swift-actions:
	cargo run -q --locked --offline -p echode-actions --example swift_actions -- --check swift/EchodeBridge/Sources/EchodeBridge/EditAction.swift

check-format:
	cargo fmt --all --check

# The verifier records the Rust part, the Swift part and product contracts as separate checks.
check-base:
	$(MAKE) bundled-plugins
	python3 -B scripts/check_build_host.py
	cargo build -q --workspace
	cargo clippy -q --workspace --all-targets -- -D warnings
	$(MAKE) check-swift-actions
	cargo test -q --workspace
	cargo clippy -q -p echode-history --features spike --all-targets -- -D warnings
	cargo test -q -p echode-history --features spike --lib --test roundtrip
	cargo build -q -p echode-engine --features spike --bin echode-engine-spike
	cargo clippy -q -p echode-engine --features spike --all-targets -- -D warnings
	cargo test -q -p echode-engine --features spike --lib
	scripts/check-ffi.sh
	python3 -B scripts/check_audio_lints.py
	@command -v cargo-deny >/dev/null || { echo "cargo-deny is missing: run make setup"; exit 1; }
	cargo deny -L error check licenses bans sources

# Swift tests start the plugin helper, the test plugin and the CLI, so a Swift-only
# change can reuse check-base but must still build what these tests run.
check-swift:
	$(MAKE) bundled-plugins
	python3 -B scripts/check_build_host.py
	cargo build -q --locked --offline -p echode-ffi -p echode-plugin-host -p echode-test-plugin -p echode-cli
	$(MAKE) check-shell
	$(MAKE) check-bridge

# Each required contract, its tests and its counterexample are listed in scripts/verification.json.
check-product-contracts:
	$(MAKE) contract-prerequisites
	PYTHONDONTWRITEBYTECODE=1 python3 scripts/check_product_contracts.py --changed-controls

# Every counterexample, including those whose only change is a linked dependency (ADR 0212).
check-contract-controls:
	$(MAKE) contract-prerequisites
	PYTHONDONTWRITEBYTECODE=1 python3 scripts/check_product_contracts.py

contract-prerequisites:
	$(MAKE) bundled-plugins
	cargo build -q --locked --offline -p echode-ffi -p echode-plugin-host -p echode-test-plugin -p echode-cli
	set -eu; header="$$(find target/debug/build -path '*echode-ffi-*/out/echode.h' -print0 | xargs -0 ls -t | head -1)"; \
	test -n "$$header"; \
	mkdir -p swift/EchodeBridge/CEchodeFfi/include; \
	cmp -s "$$header" swift/EchodeBridge/CEchodeFfi/include/echode.h || cp "$$header" swift/EchodeBridge/CEchodeFfi/include/echode.h

# Names the EchodeApp test that reaches the real HAL. Opens no device: the test bundle has no HAL (ADR 0209).
check-real-hal:
	cargo build -q --locked --offline -p echode-ffi
	set -eu; header="$$(find target/debug/build -path '*echode-ffi-*/out/echode.h' -print0 | xargs -0 ls -t | head -1)"; \
	test -n "$$header"; \
	mkdir -p swift/EchodeBridge/CEchodeFfi/include; \
	cmp -s "$$header" swift/EchodeBridge/CEchodeFfi/include/echode.h || cp "$$header" swift/EchodeBridge/CEchodeFfi/include/echode.h
	python3 -B scripts/check_real_hal.py

# Runs every EchodeApp test one at a time and names each that reaches the real HAL (ADR 0271).
.PHONY: check-real-hal-names
check-real-hal-names:
	python3 -B scripts/check_real_hal.py --name-tests

# Selection and evidence are shared by agents, the stop hook, and CI.
verify:
	python3 scripts/verify.py $(VERIFY_ARGS)

# Every automatic check, including sanitizers and all counterexamples. Not needed per task.
verify-full:
	python3 scripts/verify.py --full $(VERIFY_ARGS)

verify-plan:
	python3 scripts/verify.py --plan $(VERIFY_ARGS)

# Diagnostics only: no test execution and no pass-cache writes.
.PHONY: verification-profile
verification-profile:
	python3 -B scripts/verification_metrics.py

check-verification:
	python3 -B scripts/run_unit_tests.py

# Static checks that need no M4, for contributors and GitHub Actions (ADR 0269).
# Hosted macOS runners are M1 / M2: clippy compiles for apple-m1 there, so build scripts and
# proc macros never execute M4 instructions. Not a substitute for `make check`.
.PHONY: check-portable
check-portable:
	cargo fmt --all --check
	python3 -B scripts/run_unit_tests.py
	scripts/check-ffi.sh
	python3 -B scripts/check_audio_lints.py
	@command -v cargo-deny >/dev/null || { echo "cargo-deny is missing: run make setup"; exit 1; }
	cargo deny -L error check licenses bans sources
	cargo clippy -q --locked --workspace --all-targets $(PORTABLE_BUILD) -- -D warnings
	cargo clippy -q --locked -p echode-history --features spike --all-targets $(PORTABLE_BUILD) -- -D warnings
	cargo clippy -q --locked -p echode-engine --features spike --all-targets $(PORTABLE_BUILD) -- -D warnings

# Appended after .cargo/config.toml's target-cpu, so rustc uses this last value.
# A separate target directory keeps a local run from invalidating the M4 build.
PORTABLE_BUILD := --target-dir target/portable --config 'target.aarch64-apple-darwin.rustflags=["-C", "target-cpu=apple-m1"]'

doctor:
	@python3 -B scripts/diagnose.py

agent-context:
	@python3 -B scripts/agent_context.py $(CONTEXT_ARGS)

debug-report:
	@python3 -B scripts/diagnose.py --bundle

fmt:
	cargo fmt

# Tools `make check` needs besides the toolchain in rust-toolchain.toml.
setup:
	cargo install --locked cargo-deny@0.20.2

# Fixed validation toolchain; never changes the product compiler/default.
setup-bridge:
	bash scripts/setup-bridge.sh

# Rewrites crates/echode-cli/tests/fixtures/sine-pad-a4-128.wav and appends REASON
# to tests/fixtures/bless.log. Use this when a sound change is intentional.
bless:
	@test -n "$(REASON)" || { echo 'usage: make bless REASON="why the reference changed"'; exit 1; }
	BLESS=1 cargo test -p echode-cli --test render -- --ignored bless_the_reference --exact
	mkdir -p crates/echode-cli/tests/fixtures
	printf '%s %s\n' "$$(date +%Y-%m-%d)" "$(REASON)" >> crates/echode-cli/tests/fixtures/bless.log

# Ignored audit_* tests reproduce open items in docs/reviews/.
# Each one fails until its item is fixed; then remove its #[ignore].
known-bugs:
	cargo test --workspace --no-fail-fast -- --ignored audit_

# Advisories need the network, so they are not part of `make check`.
advisories:
	cargo deny check advisories

# Release measurement of cross-process plugin hosting. See docs/spikes/S3-plugin-isolation.md.
spike-isolation:
	bash scripts/build-bundled-plugins.sh release
	cargo build -q --release -p echode-plugin-host -p echode-test-plugin
	cargo test -q --release -p echode-plugin-host --test isolation -- --include-ignored --test-threads=1
	cargo test -q --release -p echode-plugin-host --test hosted_playback -- --include-ignored --test-threads=1

# Metal scroll/zoom measurement. See docs/spikes/S1-drawing.md.
spike-draw:
	cargo build -q --release -p echode-draw
	cargo test -q --release -p echode-draw
	cd spikes/draw && swift run -c release EchodeDraw --bench --report ../../target/spike-s1.txt

# Product arrange query measurement, without a window or audio device.
.PHONY: spike-arrange-query
spike-arrange-query:
	cargo test -q --release -p echode-draw --test arrange_pitch measure_product_arrange_query -- --ignored --exact --nocapture

# Plain-text project history. See docs/spikes/S4-history.md.
spike-history:
	cargo test -q --release -p echode-history --features spike --test roundtrip -- --include-ignored --test-threads=1
	cargo test -q --release -p echode-model --lib a_gain_edit_stays_cheap_as_unrelated_notes_grow -- --ignored

# Trackpad pinch, haptics, and chord pressure. See docs/spikes/S5-input.md.
spike-input:
	$(MAKE) check-input
	cd spikes/input && swift run

check-input:
	swift test -q --package-path spikes/input --scratch-path target/input-tests

# Timing belongs in release measurements, not in the normal correctness checks.
spike-render:
	cargo test -q --release -p echode-cli --test render a_minute_of_stereo_silence_is_written_in_bulk -- --ignored --exact
	cargo test -q --release -p echode-cli --lib dither_tests::a_minute_of_pcm_with_and_without_tpdf_is_written_in_bulk -- --ignored --exact --nocapture

# UI-2: regenerate UniFFI and cbindgen bindings; Swift 6 strict concurrency.
# check-bridge has no timing assertions. spike-bridge also prints measurements.
check-bridge:
	bash scripts/spike-bridge.sh

# Product Swift wrapper tests through the generated C ABI. No timing assertions.
check-shell:
	bash scripts/check-shell.sh

spike-bridge:
	bash scripts/spike-bridge.sh --bench

check-bridge-features:
	cargo clippy -q --locked -p echode-bridge-spike --all-targets --all-features -- -D warnings
	cargo test -q --locked -p echode-bridge-spike --lib --all-features
	cargo deny -L error --all-features check licenses bans sources

# The product Swift tests under a sanitizer launch the plugin helper, the test plugin and the
# CLI from target/debug, as in check-swift. A fresh checkout, like GitHub's full workflow, has none.
.PHONY: sanitizer-prerequisites
sanitizer-prerequisites:
	$(MAKE) bundled-plugins
	cargo build -q --locked --offline -p echode-plugin-host -p echode-test-plugin -p echode-cli

check-bridge-asan:
	$(MAKE) sanitizer-prerequisites
	bash scripts/check-bridge-sanitizer.sh address

check-bridge-tsan:
	$(MAKE) sanitizer-prerequisites
	bash scripts/check-bridge-sanitizer.sh thread

# No Echode code: require the errno lifetime fix and detection of a real race.
check-tsan-runtime:
	bash scripts/check-tsan-runtime.sh

check-bridge-miri:
	bash scripts/check-bridge-miri.sh

# Brief hardware check of the same 1,000-node graph; not a long-run stability claim.
spike-engine-smoke:
	cargo build -q --release -p echode-engine --features spike
	cargo run -q --release -p echode-engine --features spike --bin echode-engine-spike -- --seconds 10 --native-rate --workers auto --report target/spike-s2-smoke.txt

# Explicit long-run stability and release event-sort measurement. See docs/spikes/S2-engine.md.
spike-engine:
	cargo build -q --release -p echode-engine --features spike
	cargo test -q --release -p echode-engine --features spike -- --include-ignored --skip audit_
	cargo run -q --release -p echode-engine --features spike --bin echode-engine-spike -- --seconds 600 --workers auto --report target/spike-s2.txt

# RealtimeSanitizer on the test synth, plan handoff, and the plugin helper.
# The first build downloads the prebuilt library with curl. See docs/reviews/2026-10-08-audit.md (ENG-12).
rtsan:
	$(MAKE) bundled-plugins
	RTSAN_ENABLE=1 cargo test -q -p echode-engine --features rtsan under_rtsan
	RTSAN_ENABLE=1 cargo test -q -p echode-audio-io --features rtsan under_rtsan
	RTSAN_ENABLE=1 cargo test -q -p echode-plugin-host --features rtsan under_rtsan
	# The RTSan helper replaced target/debug/echode-plugin-host and cannot load without the
	# sanitizer runtime; leave the normal helper for the CLI and later builds.
	cargo build -q -p echode-plugin-host

# Device-free release measurement of the callback's own CPU time as plugin
# and note-heavy tracks grow, including wraps and edits. See ADR 0236 / 0239.
spike-plugin-tracks:
	cargo build -q --release -p echode-plugin-host -p echode-test-plugin
	cargo test -q --release -p echode-plugin-host --test track_scale -- --ignored --exact measure_callback_cost_as_tracks_grow --nocapture

# Device-free release measurement of blocks and edits that grow with the score.
# See docs/reviews/2026-11-03-full-code-audit.md (FCA-01 to FCA-05).
spike-scale:
	cargo test -q --release -p echode-engine --test scale -- --ignored --exact measure_playback_scale --nocapture

# Device-free release measurement; never part of normal correctness tests.
spike-audition:
	cargo test -q --release -p echode-audio-io --lib audition::tests::measure_streaming_audition_startup_without_a_device -- --ignored --exact

# Synthetic failure latency comparison through the existing verifier; no product/device work.
spike-verification-failures:
	python3 -B scripts/measure_verification_failures.py


# Product HAL callback/lifecycle probe. Writes silence and never opens a window.
spike-audio:
	cargo test -q --release -p echode-audio-io -- --ignored
