# Propagate every recipe/pipeline failure, including multi-command recipes.
# Apple's GNU make 3.81 does not support .SHELLFLAGS; pass options in SHELL.
SHELL := /bin/bash -euo pipefail
.PHONY: spike-verification-failures

.PHONY: bundled-plugins app dist xcode sign-setup spike-app spike-playhead spike-arrange-frame spike-piano-frame spike-audio spike-audition check check-format check-base check-product-contracts verify verify-plan check-verification doctor agent-context debug-report fmt setup setup-bridge known-bugs advisories bless spike-isolation spike-draw spike-history spike-input check-input spike-engine spike-engine-smoke spike-bridge spike-render check-bridge check-bridge-features check-bridge-asan check-bridge-tsan check-tsan-runtime check-bridge-miri check-shell rtsan

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
	cp "$$header" swift/EchodeBridge/CEchodeFfi/include/echode.h; \
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
	$(MAKE) check-product-contracts

check-format:
	cargo fmt --all --check

# The verifier records this part and product contracts as separate checks.
check-base:
	$(MAKE) bundled-plugins
	python3 -B scripts/check_build_host.py
	cargo build -q --workspace
	cargo clippy -q --workspace --all-targets -- -D warnings
	cargo test -q --workspace
	scripts/check-ffi.sh
	@command -v cargo-deny >/dev/null || { echo "cargo-deny is missing: run make setup"; exit 1; }
	cargo deny -L error check licenses bans sources
	$(MAKE) check-shell
	$(MAKE) check-bridge

# Audition streaming/EOF, resident output, volume and manual preview contracts use this entry too.
# Named behavioral tests (including independent workspace panes/stacking, canvas resizing/header width/stale drawable extents and margins, fixed transport chrome sizes, the remembered playback start and stop return, output prewarm/background media, background graph startup, device preparation/disclosure/drop targets, plugin window lifetimes/status/keys, tracks, FX, and file-URL sample drops) must run; ignored/deleted tests cannot pass.
# Arrange/piano ruler clicks, loop drags/edge handles, cancellation and shared row geometry and loop entry/bypass run through the same native/RT contract gate.
# Content-relative auto moves/copies, subsequent refits, and unmarked legacy journals share this gate.
# Arrange-local Command-D repeats the exact current clip window with independent content and atomic history.
# Blank piano-grid clicks/drags, remembered input length with Undo/Redo, release geometry and cancellation share the native UI gate.
# Shared piano viewports across track switches and document-lifetime guards use this native UI gate too.
# Right-drag note selection, Command toggles, atomic deletion and linked-content deduplication also use this gate.
# Group note movement, common bounds, linked-content writes and gesture cancellation share this gate.
# Group edge resizing shares common one-tick/time bounds, previews and atomic history through this gate.
# Independent piano targets, reference-only notes, track controls and ordered arrow/Return routing share this gate too.
# No-op commands/groups preserve history, durable publication and MCP acknowledgements through this gate.
# Command note copies preserve full-resolution attributes, linked identity, selection and atomic history through this gate.
# Piano automatic/fixed straight/triplet/dotted grid controls, zoom anchoring, snap subdivisions and press cancellation share this gate.
# Piano time guides retain beat/four-quarter references independently of snap subdivisions and thinning.
# Selected-track mute/solo switches capture event targets, serialize current-state toggles and share atomic history through this gate.
# Selected-track numeric entries and native accessibility steps share common limits, current-state ordering and atomic history.
# Zero-horizontal-travel header drags preserve exact gains below the display floor and leave Undo/Redo untouched.
# Clip identity drags preserve exact gain/fade values and Redo; package floats retain every nonzero bit across disk saves.
# Canonical PCM rejects non-finite samples before trimmed clip preparation or SRC, without rewriting stored objects.
# Peak cache allocations stay within file data; invalid pyramid shapes or columns rebuild from canonical PCM.
# Pending resets keep press state only for the matching header control or clip handle; cross-target edits retain separate history.
# Native scan fixtures prove the direct child has emitted its report and exited before the held-pipe deadline.
# Sanitizer entry points also run recovery consent separately, retaining its tests and readiness deadline.
check-product-contracts:
	$(MAKE) bundled-plugins
	cargo build -q --locked --offline -p echode-ffi -p echode-plugin-host -p echode-test-plugin -p echode-cli
	set -eu; header="$$(find target/debug/build -path '*echode-ffi-*/out/echode.h' -print0 | xargs -0 ls -t | head -1)"; \
	test -n "$$header"; \
	mkdir -p swift/EchodeBridge/CEchodeFfi/include; \
	cp "$$header" swift/EchodeBridge/CEchodeFfi/include/echode.h
	PYTHONDONTWRITEBYTECODE=1 python3 scripts/check_product_contracts.py

# Selection and evidence are shared by agents, the stop hook, and CI.
verify:
	python3 scripts/verify.py $(VERIFY_ARGS)

verify-plan:
	python3 scripts/verify.py --plan $(VERIFY_ARGS)

# Diagnostics only: no test execution and no pass-cache writes.
.PHONY: verification-profile
verification-profile:
	python3 -B scripts/verification_metrics.py

check-verification:
	PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover -s scripts/tests

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
	cargo test -q --release -p echode-history --test roundtrip -- --include-ignored --test-threads=1
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

# UI-2: regenerate UniFFI and cbindgen bindings; Swift 6 strict concurrency.
# check-bridge has no timing assertions. spike-bridge also prints measurements.
check-bridge:
	bash scripts/spike-bridge.sh

# Product Swift wrapper, piano gestures/grids/time guides, single/batch velocity, note selection, batch deletion/movement/resizing/copying. No timing assertions.
# Includes selected-track numeric/accessible mix batches and header/clip gesture ownership across document replacement.
check-shell:
	bash scripts/check-shell.sh

spike-bridge:
	bash scripts/spike-bridge.sh --bench

check-bridge-features:
	cargo clippy -q --locked -p echode-bridge-spike --all-targets --all-features -- -D warnings
	cargo test -q --locked -p echode-bridge-spike --lib --all-features
	cargo deny -L error --all-features check licenses bans sources

check-bridge-asan:
	bash scripts/check-bridge-sanitizer.sh address

check-bridge-tsan:
	bash scripts/check-bridge-sanitizer.sh thread

# No Echode code: require the errno lifetime fix and detection of a real race.
check-tsan-runtime:
	bash scripts/check-tsan-runtime.sh

check-bridge-miri:
	bash scripts/check-bridge-miri.sh

# Brief hardware check of the same 1,000-node graph; not a long-run stability claim.
spike-engine-smoke:
	cargo build -q --release -p echode-engine
	cargo run -q --release -p echode-engine --bin echode-engine-spike -- --seconds 10 --native-rate --workers auto --report target/spike-s2-smoke.txt

# Explicit long-run stability and release event-sort measurement. See docs/spikes/S2-engine.md.
spike-engine:
	cargo build -q --release -p echode-engine
	cargo test -q --release -p echode-engine -- --include-ignored --skip audit_
	cargo run -q --release -p echode-engine --bin echode-engine-spike -- --seconds 600 --workers auto --report target/spike-s2.txt

# RealtimeSanitizer on the test synth, plan handoff, and the plugin helper.
# The first build downloads the prebuilt library with curl. See docs/reviews/2026-10-08-audit.md (ENG-12).
rtsan:
	$(MAKE) bundled-plugins
	RTSAN_ENABLE=1 cargo test -q -p echode-engine --features rtsan under_rtsan
	RTSAN_ENABLE=1 cargo test -q -p echode-audio-io --features rtsan under_rtsan
	RTSAN_ENABLE=1 cargo test -q -p echode-plugin-host --features rtsan under_rtsan

# Device-free release measurement; never part of normal correctness tests.
spike-audition:
	cargo test -q --release -p echode-audio-io --lib audition::tests::measure_streaming_audition_startup_without_a_device -- --ignored --exact

# Synthetic failure latency comparison through the existing verifier; no product/device work.
spike-verification-failures:
	python3 -B scripts/measure_verification_failures.py


# Product HAL callback/lifecycle probe. Writes silence and never opens a window.
spike-audio:
	cargo test -q --release -p echode-audio-io -- --ignored
