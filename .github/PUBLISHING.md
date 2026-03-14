# Publishing Guide for unitree-webrtc-rs

## PyPI 배포 워크플로우

### 1. GitHub Secrets 설정

Repository Settings → Secrets and variables → Actions에서 다음 secrets 추가:

#### PyPI (Production)

- `PYPI_API_TOKEN`: PyPI API 토큰
  - https://pypi.org/manage/account/token/ 에서 생성
  - Scope: "Entire account" 또는 특정 프로젝트

### 2. 배포 워크플로우

#### 자동 배포 (Production PyPI)

**Trigger**: Git tag push

```bash
# 버전 태그 생성 및 푸시
git tag v0.1.0
git push origin v0.1.0
```

**Process**:

1. Multi-platform 빌드 (Ubuntu, macOS Intel, macOS ARM64)
2. Python 3.12용 wheels 생성
3. PyPI에 자동 publish
4. `pip install unitree-webrtc-rs` 가능

#### 테스트 배포 (TestPyPI)

**Trigger**: Manual workflow dispatch

**Steps**:

1. GitHub Actions → "Build and Test (TestPyPI)" workflow 선택
2. "Run workflow" 클릭
3. TestPyPI에 배포됨

**Install from TestPyPI**:

```bash
pip install --index-url https://test.pypi.org/simple/ unitree-webrtc-rs
```

### 3. 로컬 빌드 (개발용)

```bash
# 의존성 동기화
uv sync

# 개발 모드 설치
uv run maturin develop

# Release 빌드 (wheel 생성)
uv run maturin build --release --out dist

# 로컬 설치
pip install dist/unitree_webrtc_rs-*.whl
```

### 4. 버전 관리

`pyproject.toml`에서 버전 업데이트:

```toml
[project]
version = "0.1.0"  # 이 버전을 변경
```

`Cargo.toml`에서도 버전 동기화:

```toml
[package]
version = "0.1.0"  # pyproject.toml과 동일하게 유지
```

### 5. 배포 체크리스트

- [ ] 버전 업데이트 (`pyproject.toml`, `Cargo.toml`)
- [ ] CHANGELOG.md 업데이트
- [ ] `cargo fmt` 실행
- [ ] `cargo clippy` 검증 (0 warnings)
- [ ] 로컬에서 `maturin build` 성공 확인
- [ ] Git commit & push
- [ ] Git tag 생성 (`git tag v0.1.0`)
- [ ] Tag push (`git push origin v0.1.0`)
- [ ] GitHub Actions workflow 성공 확인
- [ ] PyPI에서 패키지 확인

### 6. Workflow 파일

#### `.github/workflows/publish-pypi.yml`

- Production PyPI 배포
- Tag push 시 자동 실행
- Multi-platform wheel 빌드

#### `.github/workflows/test-pypi.yml`

- CI 테스트 + TestPyPI 배포
- PR/push 시 빌드 테스트
- Manual trigger로 TestPyPI 배포

### 7. 시스템 의존성

Workflow에서 자동 설치되는 패키지:

- **Ubuntu**: `libgstreamer1.0-dev`, `libgstreamer-plugins-base1.0-dev`, `libopus-dev`, `pkg-config`
- **macOS**: `gstreamer`, `gst-plugins-base`, `opus`, `pkg-config`

### 8. 트러블슈팅

#### "No module named 'unitree_webrtc_rs'"

```bash
# 빌드 후 재설치
uv run maturin develop --release
```

#### "Rust compilation failed"

```bash
# Rust 도구체인 업데이트
rustup update stable
```

#### "GStreamer not found"

```bash
# Ubuntu
sudo apt-get install libgstreamer1.0-dev libgstreamer-plugins-base1.0-dev

# macOS
brew install gstreamer gst-plugins-base
```

### 9. 환경별 설치 방법

#### 최종 사용자 (PyPI)

```bash
pip install unitree-webrtc-rs
```

#### 개발자 (로컬)

```bash
git clone <repo>
cd unitree-webrtc-rs
uv sync
uv run maturin develop
```

#### Jetson (ARM64)

```bash
# 시스템 의존성 설치 필요
sudo apt-get install libgstreamer1.0-dev libgstreamer-plugins-base1.0-dev libopus-dev

# PyPI에서 설치 (ARM64 wheel 빌드되는 경우)
pip install unitree-webrtc-rs

# 또는 소스에서 빌드
git clone <repo>
cd unitree-webrtc-rs
pip install maturin
maturin build --release
pip install dist/*.whl
```
