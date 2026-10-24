# Publishing Guide for unitree-webrtc-rs

## CI와 배포 기준

[acceptance.yml](workflows/acceptance.yml)은 fork를 포함한 PR과 `main` push에서 **배포 없이** locked `make acceptance`를 실행한다. PR이 열린 feature branch의 push는 PR event로만 검증하여 중복 push 실행을 만들지 않는다. PR이 없는 feature branch는 acceptance가 자동 실행되지 않으므로, 로컬에서 `make acceptance`를 실행한 뒤 PR을 열어 hosted checks를 시작한다. `contents: read`만 사용하고 checkout credential을 유지하지 않으며, 같은 PR/ref의 이전 acceptance 실행은 취소한다. Release workflow도 같은 전체 reusable job/matrix를 호출하므로 검증 절차를 별도로 복제하지 않는다.

[publish-pypi.yml](workflows/publish-pypi.yml)은 버전 태그 `v*.*.*` push 또는 manual `workflow_dispatch`로 시작한다. 두 trigger 모두 production PyPI 배포 경로다. Manual 실행은 **선택한 ref를 배포**하며 태그가 필수는 아니다. TestPyPI 검사나 dry run으로 사용하지 않는다. 동일 ref의 release는 upload 도중 취소하지 않고 순서대로 실행한다.

| 항목 | 현재 구성 |
| --- | --- |
| OS | 기존 `ubuntu-latest`, `macos-14` runner matrix |
| Python | 3.10, 3.11, 3.12 |
| Rust / uv | `rust-toolchain.toml`의 channel/components / uv 0.7.19 |
| Jetson aarch64 | 빌드 matrix에 없음; 별도 source build/검증 필요 |
| 태그 release gate | 태그 `v<version>` = Cargo/pyproject/local lock project versions |
| Manual release gate | 동일한 네 project versions 검사; 태그 비교는 생략 |
| 공통 빌드 전 gate | locked development sync → project version 검사 → `make acceptance` |
| Release wheel gate | locked release build → 실제 wheel METADATA name/version 검사 → 저장소 밖 fresh install/native tests |
| 배포 | 모든 matrix job 성공 후 `pypi` environment에서 PyPI 업로드 |

[scripts/check-release.py](../scripts/check-release.py)는 `Cargo.toml`, `pyproject.toml`, `Cargo.lock`, `uv.lock`의 **현재 프로젝트** 항목을 파싱한다. 다른 dependency version은 비교 대상이 아니다. `--tag v<version>`을 전달하면 정확한 태그 일치를 요구하며, `--wheel path.whl`은 filename 대신 실제 `.dist-info/METADATA`의 name/version을 검증하고 SHA-256을 출력한다. 실패 시 수정/게시 없이 nonzero로 종료한다.

Fresh wheel smoke는 임시 venv에 wheel과 기존 locked 개발 환경의 pytest/pytest-asyncio 버전을 설치한다. 실제 extension loader, 설치 경로, shipped stub, legacy aliases를 검사한 뒤 checkout에서 아래 **offline native subset만** 임시 폴더로 복사해 실행한다. 테스트나 fixture가 wheel에 포함된다고 가정하지 않으며 develop artifact나 저장소 import 경로를 사용하지 않는다.

- `tests/test_native_contracts.py`
- `tests/test_native_logging.py`
- `tests/test_native_input_contracts.py`
- `tests/test_native_callback_lifetime.py`

Workflow 구성은 CI 성공이나 로봇 검증의 증거가 아니다. 위 테스트는 로봇에 연결하지 않는다. Fresh install도 native library 독립 배포성을 보증하지 않으며, GStreamer/Opus 등 시스템 의존성이 필요하다. 설치 항목은 [README](../README.md#system-dependencies)와 workflow를 확인한다.

## Artifact와 provenance

각 matrix job은 wheel을 `wheels-<os>-<python>` artifact에, source commit/workflow ref/run ID, 실제 Rust/Cargo/uv/Python/maturin 버전과 wheel SHA-256을 별도 `wheel-evidence-<os>-<python>` artifact에 보관한다. Publish job은 `wheels-*`만 다운로드하므로 evidence text를 PyPI에 업로드하지 않는다. 보관 기간은 7일이다. 이 기록은 서명된 attestation이나 reproducible-build 증명이 아니다.

Actions는 upstream의 기존 major/release ref를 조회해 확인한 immutable commit에 고정한다. 갱신 시 official upstream에서 ref와 peeled commit을 다시 확인하고 diff를 검토한다. Runner image/system package는 완전히 고정되어 있지 않다.

## 인증과 외부 production controls

Repository Actions secret `PYPI_API_TOKEN`에 해당 프로젝트용 PyPI token을 설정한다. 현재 publish action은 이 token을 `password`로 전달하며 `attestations: false`다. Publish job에는 `id-token: write` permission도 있지만, 현재 구성을 OIDC-only trusted publishing으로 설명하거나 token을 임의로 제거하지 않는다. 인증 방식 변경은 별도 검토 대상이다. Token을 문서, 명령 인자, 로그에 기록하지 않는다.

Branch protection/required acceptance checks, `pypi` environment 승인자와 허용 ref, repository/tag 권한, PyPI token scope/rotation 또는 trusted-publisher 설정은 repository/PyPI 관리자 소유다. 이 소스 변경으로 설정하거나 실행 여부를 검증하지 않았다. Manual production 실행의 승인 정책과 signed provenance 전환도 별도 권한/검토가 필요하다.

## 버전과 로컬 검증

[버전 helper](../scripts/sync-version.sh)는 `Cargo.toml`, `pyproject.toml`, `Cargo.lock`, `uv.lock`을 함께 갱신하고 실패 시 네 파일을 복원한다. Dependency 전체 upgrade 대신 기존 lock 갱신을 사용한다. 네 파일의 diff와 예상하지 않은 dependency 변경을 확인한다. Helper 사용법은 `./scripts/sync-version.sh <major.minor.patch>`다.

- 초기 개발 환경은 `make init`; 기존 환경의 검증은 `make acceptance`를 사용한다.
- `make check-release`는 현재 네 project versions를 검증한다. 태그를 확인하려면 `uv run --locked --no-sync python scripts/check-release.py --tag v<version>`을 실행한다.
- `make deploy VERSION=<major.minor.patch>`는 버전 동기화와 locked release wheel **빌드**만 수행하며 publish하지 않는다.
- 실제 wheel metadata는 `uv run --locked --no-sync python scripts/check-release.py --wheel /absolute/path/package.whl`로 검증한다.
- `make wheel-smoke WHEEL=/absolute/path/package.whl`은 저장소 밖 임시 환경에서 fresh install/native subset을 실행한다. 시스템 media 의존성과 wheel/test 의존성을 설치할 수 있는 환경이 필요하다.
- Application checks와 native prerequisites는 [CONTRIBUTING](../CONTRIBUTING.md#setup-and-verification)을 따른다.

## 배포 전 확인

1. 승인된 release 범위/버전과 태그 또는 manual ref를 확인한다.
2. Version gate, `make acceptance`, 실제 wheel metadata/fresh smoke 결과와 artifact digest를 기록한다. Build만으로 native test 성공을 주장하지 않는다.
3. Proprietary 라이선스, upstream notices와 배포 의무를 확인한다. 이 절차는 OSS 전환 승인이 아니다.
4. Commit/push/tag/publish는 별도 승인 범위에서만 수행한다. PR 생성이나 merge는 자체로 publish가 아니다.
5. 승인된 실행 후 실제 matrix/gates와 PyPI 결과를 직접 확인한다. 실패를 우회하거나 확인하지 않은 플랫폼을 지원한다고 표시하지 않는다.
