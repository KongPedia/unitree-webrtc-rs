# Publishing Guide for unitree-webrtc-rs

## 배포 기준

실제 설정은 [publish-pypi.yml](workflows/publish-pypi.yml)이 기준이다. 버전 태그 `v*.*.*` push 또는 manual `workflow_dispatch`만 배포를 시작하며, PR/일반 branch push는 이 workflow를 실행하지 않는다. 두 trigger 모두 production PyPI 배포 경로이므로 manual 실행을 TestPyPI 검사로 사용하지 않는다.

| 항목 | 현재 구성 |
| --- | --- |
| OS | `ubuntu-latest` (x86_64), `macos-14` (Apple Silicon) |
| Python | 3.10, 3.11, 3.12 |
| Jetson aarch64 | 빌드 matrix에 없음; 별도 source build/검증 필요 |
| 빌드 전 gate | locked development sync → `make acceptance` |
| wheel gate | locked release wheel → 저장소 밖 임시 환경에서 fresh import → artifact upload |
| 배포 | 모든 matrix job 성공 후 `pypi` environment에서 PyPI 업로드 |

Workflow가 구성되어 있다는 사실은 CI 실행 성공이나 로봇 검증의 증거가 아니다. Fresh import는 wheel의 실제 extension loader, stub, legacy aliases를 검사하지만 native library 독립 배포성을 보증하지 않는다. 각 환경에는 GStreamer/Opus 등 시스템 의존성이 필요하다. 설치 항목은 [README](../README.md#system-dependencies)와 workflow를 확인한다.

## 인증

Repository Actions secret `PYPI_API_TOKEN`에 해당 프로젝트용 PyPI token을 설정한다. 현재 publish action은 이 token을 `password`로 전달하며 `attestations: false`다. Publish job에는 `id-token: write` permission도 있지만, 현재 구성을 OIDC-only trusted publishing으로 설명하거나 token을 임의로 제거하지 않는다. 인증 방식 변경은 별도 검토 대상이다. Token을 문서, 명령 인자, 로그에 기록하지 않는다.

## 버전과 로컬 검증

[버전 helper](../scripts/sync-version.sh)는 `Cargo.toml`, `pyproject.toml`, `Cargo.lock`, `uv.lock`을 함께 갱신하고 실패 시 네 파일을 복원한다. Dependency 전체 upgrade 대신 기존 lock 갱신을 사용한다. 네 파일의 diff와 예상하지 않은 dependency 변경을 확인한다. Helper 사용법은 `./scripts/sync-version.sh <major.minor.patch>`다.

- 초기 개발 환경은 `make init`; 기존 환경의 검증은 `make acceptance`를 사용한다.
- `make deploy VERSION=<major.minor.patch>`는 버전 동기화와 locked release wheel **빌드**만 수행하며 publish하지 않는다.
- 생성한 실제 wheel 경로를 `make wheel-smoke WHEEL=/absolute/path/package.whl`에 전달한다. 저장소 밖 임시 환경에서 설치/import하며 로봇에 접속하지 않는다.
- Application checks와 native prerequisites는 [CONTRIBUTING](../CONTRIBUTING.md#setup-and-verification)을 따른다. 이 문서 정리만으로 release build/배포를 실행하지 않는다.

## 배포 전 확인

1. 명시적으로 승인된 release 범위와 버전, 네 version file의 일치를 확인한다.
2. `make acceptance`와 실제 fresh wheel smoke의 실행 결과를 기록한다. Build 성공만으로 native import 성공을 주장하지 않는다.
3. Proprietary 라이선스, upstream notices와 배포 의무를 확인한다. 이 절차는 OSS 전환 승인이 아니다.
4. Commit/push/tag/publish는 별도 승인 범위에서만 수행한다. PR 생성이나 merge는 자체로 publish가 아니다.
5. 승인된 tag 또는 manual 실행 후 matrix/gates와 PyPI 결과를 직접 확인한다. 실패를 우회하거나 확인하지 않은 플랫폼을 지원한다고 표시하지 않는다.

위 명령/경로는 현재 소스와 설정을 확인한 사용법이며, 문서 정리 과정에서 release mutation, 빌드 또는 외부 게시를 재실행한 결과가 아니다.
