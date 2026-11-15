# The recipes live in the Makefile. `just <recipe>` runs the make target of the same name.

bundled-plugins:
    make bundled-plugins

app:
    make app

dist:
    make dist

xcode:
    make xcode

sign-setup:
    make sign-setup

spike-app:
    make spike-app

spike-playhead:
    make spike-playhead

spike-arrange-frame:
    make spike-arrange-frame

spike-piano-frame:
    make spike-piano-frame

spike-main-thread:
    make spike-main-thread

check:
    make check

swift-actions:
    make swift-actions

check-swift-actions:
    make check-swift-actions

check-format:
    make check-format

check-lint:
    make check-lint

check-base:
    make check-base

check-swift:
    make check-swift

check-spikes:
    make check-spikes

check-product-contracts:
    make check-product-contracts

check-contract-controls:
    make check-contract-controls

contract-prerequisites:
    make contract-prerequisites

check-real-hal:
    make check-real-hal

check-real-hal-names:
    make check-real-hal-names

verify:
    make verify

verify-full:
    make verify-full

verify-plan:
    make verify-plan

wait-verify:
    make wait-verify

verification-profile:
    make verification-profile

check-verification:
    make check-verification

check-portable:
    make check-portable

doctor:
    make doctor

agent-context:
    make agent-context

debug-report:
    make debug-report

fmt:
    make fmt

setup:
    make setup

setup-lint:
    make setup-lint

setup-bridge:
    make setup-bridge

setup-fuzz:
    make setup-fuzz

fuzz target seconds="300":
    make fuzz FUZZ_TARGET="{{target}}" FUZZ_SECONDS="{{seconds}}"

check-fuzz:
    make check-fuzz

setup-coverage:
    make setup-coverage

coverage:
    make coverage

bless reason:
    make bless REASON="{{reason}}"

known-bugs:
    make known-bugs

advisories:
    make advisories

spike-isolation:
    make spike-isolation

spike-draw:
    make spike-draw

spike-arrange-query:
    make spike-arrange-query

spike-history:
    make spike-history

spike-input:
    make spike-input

check-input:
    make check-input

spike-render:
    make spike-render

check-bridge:
    make check-bridge

spike-bridge:
    make spike-bridge

check-bridge-features:
    make check-bridge-features

sanitizer-prerequisites:
    make sanitizer-prerequisites

check-bridge-asan:
    make check-bridge-asan

check-bridge-tsan:
    make check-bridge-tsan

check-tsan-runtime:
    make check-tsan-runtime

check-bridge-miri:
    make check-bridge-miri

spike-engine-smoke:
    make spike-engine-smoke

spike-engine:
    make spike-engine

rtsan:
    make rtsan

spike-plugin-tracks:
    make spike-plugin-tracks

spike-scale:
    make spike-scale

spike-import:
    make spike-import

spike-audition:
    make spike-audition

spike-verification-failures:
    make spike-verification-failures

spike-audio:
    make spike-audio
