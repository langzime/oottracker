function ThrowOnNativeFailure {
    if (-not $?)
    {
        throw 'Native Failure'
    }
}

$env:PYO3_PYTHON = "python"

#cargo test --package=oottracker
cargo check --package=oottracker #TODO replace with above
ThrowOnNativeFailure

cargo check --package=oottracker-web --package=oottracker-csharp
ThrowOnNativeFailure

cargo check --package=oottracker-bizhawk --package=oottracker-gui
ThrowOnNativeFailure
