# The recipes live in the Makefile. `just <recipe>` runs the make target of the same name.

check:
    make check

verify:
    make verify

verify-plan:
    make verify-plan

fmt:
    make fmt

setup:
    make setup

known-bugs:
    make known-bugs

bless reason:
    make bless REASON="{{reason}}"

advisories:
    make advisories

spike-isolation:
    make spike-isolation

spike-draw:
    make spike-draw

spike-history:
    make spike-history

spike-input:
    make spike-input

spike-engine:
    make spike-engine
