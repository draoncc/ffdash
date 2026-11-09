# Integration Test Suite

Comprehensive test coverage for ffdash, including property-based testing and end-to-end FFmpeg validation.

## Test Structure

`tests/*.rs` at the top level are separate test binaries; `integration.rs` pulls in the modules under `integration/`.

```
tests/
├── README.md                   # This file
├── integration.rs              # Entry point for integration/*.rs
├── common/                     # Shared utilities (config builders, assertions, FFmpeg runner)
├── integration/                # Command generation, profile round-trips, config state, FFmpeg E2E, ...
├── snapshot_commands.rs        # insta snapshots of built FFmpeg commands (tests/snapshots/)
├── encoder_selection.rs        # Encoder selection (pure, no FFmpeg)
├── dolby_vision_skip.rs        # Dolby Vision skip rules (no FFmpeg, no real file)
├── dolby_vision_e2e.rs         # Opt-in Dolby Vision check against a real sample
└── params_*.rs, validation.rs, ...
```

## Opt-in checks

These need something CI does not have. They are skipped (and pass) by default.

| Check | How to run | Needs |
|-------|-----------|-------|
| Dolby Vision end to end | set a sample env var, then `cargo test --test dolby_vision_e2e -- --nocapture` | FFmpeg and ffprobe on `PATH`, a real Dolby Vision file |
| Parameter boundary tests | `cargo test --features dev-tools boundary_value_tests -- --ignored` | FFmpeg |

### Dolby Vision end to end

`tests/dolby_vision_e2e.rs` encodes a real Dolby Vision sample with ffdash and asks ffprobe
whether the output still reports the expected Dolby Vision Profile (ADR 0003). Each case is
enabled by its own environment variable, which names the sample file. A case whose variable is
unset is skipped, so CI is unaffected.

| Environment variable | Sample | Output | Expected |
|----------------------|--------|--------|----------|
| `FFDASH_DV_SAMPLE_HEVC_MKV_P81` | Dolby Vision Profile 8.1 | HEVC (`hevc-x265`) in MKV | Profile 8.1 |
| `FFDASH_DV_SAMPLE_HEVC_MP4_P81` | Dolby Vision Profile 8.1 | HEVC (`hevc-x265`) in MP4 | Profile 8.1 |
| `FFDASH_DV_SAMPLE_AV1_MKV_P81` | Dolby Vision Profile 8.1 | AV1 (`av1-svt`) in MKV | Profile 10 |
| `FFDASH_DV_SAMPLE_HEVC_MKV_P7` | Dolby Vision Profile 7 (needs `dovi_tool` on `PATH`) | HEVC (`hevc-x265`) in MKV | Profile 8.1 |
| `FFDASH_DV_SAMPLE_HEVC_MP4_P7` | Dolby Vision Profile 7 (needs `dovi_tool` on `PATH`) | HEVC (`hevc-x265`) in MP4 | Profile 8.1 |
| `FFDASH_DV_SAMPLE_AV1_MKV_P7` | Dolby Vision Profile 7 (needs `dovi_tool` on `PATH`) | AV1 (`av1-svt`) in MKV | Profile 10 |

```bash
FFDASH_DV_SAMPLE_HEVC_MKV_P81=/path/to/profile8.1-sample.mkv   cargo test --test dolby_vision_e2e -- --nocapture
```

Use a short clip: the whole file is encoded with the software encoder (`libx265`). Without the
variable the test prints `skipping: <VAR> is not set`.

To add a case, append a row to `CASES` in `tests/dolby_vision_e2e.rs` (variable name, profile id,
container, expected Dolby Vision Profile) and add it to the table above.

## Test Categories

### 1. FFmpeg Command Generation Tests (37 tests)
**Focus:** Validate that UI settings correctly translate to FFmpeg commands

#### Unit Tests
- Rate control modes (CQ, CQCap, TwoPassVBR, CBR)
- Two-pass vs single-pass encoding
- VP9 profiles and pixel formats
- Parallelism settings (row-mt, tiles, threads)
- GOP and keyframe configurations
- Advanced tuning options
- Conditional flag handling

#### Property-Based Tests (proptest)
- CRF range validation (0-63)
- Bitrate range validation (100-50000 kbps)
- Rate control mode permutations
- CPU-used values (0-8)
- Parallelism permutations
- GOP settings (1-600 frames, lag 0-25)
- ARNR settings
- Advanced tuning combinations
- Comprehensive multi-setting tests

**Run:** `cargo test --test integration ffmpeg_commands`

### 2. Profile Workflow Tests (22 tests)
**Focus:** Ensure ConfigState ↔ Profile round-trip preserves all settings

#### Tests
- Basic profile conversion
- All rate control modes
- Two-pass settings preservation
- Parallelism settings
- GOP and keyframe settings
- Bitrate settings
- Edge cases (min/max values, auto values)

#### Property-Based Tests
- Comprehensive round-trip (13+ settings)
- ARNR settings preservation
- Advanced tuning preservation
- Extreme value handling

**Run:** `cargo test --test integration profile_workflows`

### 3. Config State Management Tests (16 tests)
**Focus:** UI state transitions and validation

#### Tests
- ConfigState initialization
- Focus navigation (Tab/Shift+Tab)
- Rate control mode switching
- Two-pass mode toggles
- Bounds checking (CRF, CPU-used, tiles, lag)
- Boolean flag toggles
- Auto/disabled value handling
- Profile modification tracking
- Value preservation during mode switches

**Run:** `cargo test --test integration config_state`

### 4. End-to-End FFmpeg Tests (16 tests) ⚡ NEW
**Focus:** Actually execute FFmpeg commands and validate output

#### Requirements
- FFmpeg must be installed and available in PATH
- Tests automatically skip if FFmpeg is not found

#### Tests
- All rate control modes with real encoding
- Quality and speed settings validation
- Parallelism settings (single vs multi-threaded)
- GOP and keyframe configurations
- Tuning options
- Edge cases (min/max quality)
- CRF value sampling (5 values)
- CPU-used value sampling (5 values)
- Built-in profiles

#### Features
- **Fast execution:** Encodes only 5 frames / 0.2 seconds
- **Automatic cleanup:** Output files deleted after test
- **Real validation:** Verifies FFmpeg exits successfully and produces output
- **Graceful degradation:** Skips tests if FFmpeg unavailable

**Run:** `cargo test --test integration ffmpeg_e2e`

## Running Tests

### Run all integration tests
```bash
cargo test --test integration
```

### Run specific test module
```bash
cargo test --test integration ffmpeg_commands
cargo test --test integration profile_workflows
cargo test --test integration config_state
cargo test --test integration ffmpeg_e2e
```

### Run specific test
```bash
cargo test --test integration test_cq_mode_generates_crf_flag
cargo test --test integration e2e_test_cq_mode_with_ffmpeg
```

### Run with output
```bash
cargo test --test integration -- --nocapture
```

### Run E2E tests only
```bash
cargo test --test integration ffmpeg_e2e
```

## Test Statistics

- **Total Tests:** 91
- **Unit Tests:** 50
- **Property-Based Tests:** 25
- **E2E Tests:** 16
- **Execution Time:** ~0.5-1.0 seconds (with FFmpeg)

## Dependencies

### Test-Only Dependencies
- `proptest` - Property-based testing framework
- `tempfile` - Temporary directory management for E2E tests
- `anyhow` - Error handling in test utilities

## E2E Test Configuration

The `FfmpegTestConfig` struct controls E2E test behavior:

```rust
FfmpegTestConfig {
    max_frames: 5,              // Encode only 5 frames
    max_duration_secs: 0.2,     // Or 0.2 seconds
    timeout_secs: 10,           // Command timeout
    keep_output: false,         // Delete output files
}
```

## How E2E Tests Work

1. **Generate test video:** Creates a small H.264 video using FFmpeg's `testsrc`
2. **Build FFmpeg command:** Converts UI config to FFmpeg command string
3. **Execute with constraints:** Runs FFmpeg with frame/duration limits
4. **Validate output:**
   - Check exit code is 0 (success)
   - Verify output file exists
   - Check output file size > 0
5. **Cleanup:** Remove output file

## Adding New Tests

### Adding a Unit Test
```rust
#[test]
fn test_my_feature() {
    let mut config = default_config();
    config.my_setting = 42;

    let cmd = build_test_cmd(&config, "MyTest");

    assert_cmd_contains(&cmd, "-my-flag 42");
}
```

### Adding a Property-Based Test
```rust
proptest! {
    #[test]
    fn proptest_my_feature(value in 0u32..=100) {
        let mut config = default_config();
        config.my_setting = value;

        let cmd = build_test_cmd(&config, "PropTest");

        assert_cmd_contains(&cmd, &format!("-my-flag {}", value));
    }
}
```

### Adding an E2E Test
```rust
#[test]
fn e2e_test_my_feature() {
    require_ffmpeg!();

    let temp_dir = TempDir::new().unwrap();
    let input = create_test_video(&temp_dir);
    let output = temp_dir.path().join("output.webm");

    let mut config = default_config();
    config.my_setting = 42;

    let cmd = build_test_cmd(&config, "E2E_MyFeature");
    let test_config = FfmpegTestConfig::default();

    let result = run_ffmpeg_command_string(&cmd, &input, &output, &test_config)
        .expect("Failed to run FFmpeg");

    assert_ffmpeg_success(&result, "My feature");
}
```

## CI/CD Integration

### GitHub Actions Example
```yaml
- name: Run integration tests
  run: cargo test --test integration

- name: Run E2E tests (requires FFmpeg)
  run: |
    sudo apt-get install -y ffmpeg
    cargo test --test integration ffmpeg_e2e
```

### Running Without FFmpeg
If FFmpeg is not available, E2E tests will be automatically skipped:
```
Skipping test: FFmpeg not available
```

## Known Issues

### CQCap Mode
The CQCap rate control mode may fail with some FFmpeg/libvpx-vp9 versions due to rate control parameter handling. The test handles this gracefully by skipping on rate control errors.

## Performance Notes

- **Unit/Property tests:** ~0.04 seconds
- **E2E tests:** ~0.5 seconds (depends on system speed)
- **Total:** ~0.6 seconds

E2E tests are optimized to encode minimal frames while still validating command correctness.
