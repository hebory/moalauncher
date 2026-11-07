# Moa Launcher

**모아 프로그램을 한곳에 모아 설치하고, 최신 버전을 확인하는 Windows 런처입니다.** GitHub Releases에서 배포자의 설치 파일을 내려받아 확인한 뒤 프로그램을 설치하고 실행합니다. 새 앱은 카탈로그에 추가하거나 앱 화면에서 등록할 수 있습니다.

## 시작하기

소스에서 실행하려면 Windows와 .NET 8 SDK가 필요합니다.

```powershell
dotnet run --project src/MoaLauncher.App
```

기본 모아뷰 카드는 [hebory/moaview](https://github.com/hebory/moaview)의 공개 GitHub Releases에 연결되어 있습니다. 실행할 때 최신 안정 버전을 확인하고 `%LOCALAPPDATA%\Programs\MoaView`의 기존 설치를 인식합니다. 다른 프로그램은 **프로그램 추가**로 등록하거나 설정 화면에서 카탈로그 JSON을 가져오세요.

설정의 **업데이트 확인 간격**에서 30분, 1시간, 3시간, 6시간, 12시간, 24시간 중 선택할 수 있습니다. 기본값은 **1시간**이며, 변경 즉시 다음 확인 간격에 적용되고 재실행해도 유지됩니다. 주기적 확인을 끄면 자동 확인 타이머가 중지됩니다. 시작할 때 확인하는 설정은 별도로 유지됩니다.

## 빌드하고 테스트하기

```powershell
dotnet build MoaLauncher.sln -c Release
dotnet test MoaLauncher.sln -c Release
```

게시용 Windows x64 ZIP과 체크섬은 다음 명령으로 만듭니다.

```powershell
./scripts/Publish.ps1
```

산출물은 `artifacts/distribution/`에 생성됩니다. 이 포터블 ZIP은 .NET 8 런타임을 포함하므로 사용자의 PC에 .NET 설치가 필요하지 않습니다. ZIP을 풀고 `MoaLauncher.exe`를 실행하세요. GitHub Action의 Windows 빌드·테스트는 커밋에서 실행되고 `v*` 태그가 게시용 ZIP, SHA-256 및 `SHA256SUMS.txt`를 GitHub Release에 첨부합니다.

## 새 앱 등록하기

앱 정의는 [catalog/apps.json](catalog/apps.json)의 목록 항목입니다. 프로그램마다 고유한 ID, 공개 `owner/repo`, 설치 파일 패턴과 실행 파일 상대 경로를 등록합니다. 여러 CPU를 지원하면 `packages`에 `x64`, `arm64`, `x86`별 형식을 추가합니다. 패턴은 릴리스 파일명에서 `*`, `?` 와일드카드를 지원하며, 일치하는 파일이 정확히 하나여야 합니다.

각 릴리스 자산은 GitHub가 API에 제공하는 `sha256:` digest 또는 자산 옆의 `<asset>.sha256`, `SHA256SUMS.txt`에서 읽을 수 있는 SHA-256이 있어야 설치할 수 있습니다. 일반 `.sha256` 파일에는 해시 하나 또는 표준 해시·파일 이름 행이 들어갈 수 있습니다. 해시를 확인할 수 없는 파일은 실행하지 않습니다. API 요청이 막히면 저장한 릴리스 결과는 표시하지만 캐시 정보만으로 설치하지 않습니다.

전체 카탈로그를 배포하려면 JSON을 GitHub Release에 첨부하고 사용자에게 신뢰할 수 있는 배포자의 JSON을 설정 화면에서 가져오도록 안내하세요. 상세 스키마와 예시는 [앱 카탈로그 안내](docs/catalog.md)에 있습니다.

## 데이터 위치와 업데이트 안전성

카탈로그 설정, 설치 상태, GitHub 캐시, 다운로드 중인 파일, 로그는 `%LOCALAPPDATA%\MoaLauncher`에 저장됩니다. 기본 카탈로그는 앱에 포함되고 사용자 카탈로그는 기본 앱 항목을 덮어쓰거나 새 앱을 추가합니다. 설정 화면에서 데이터 폴더를 열고, 앱 상세 보기에서 설치 폴더, 릴리스 정보와 실행 버튼을 찾을 수 있습니다.

포터블 ZIP은 새 버전을 별도 폴더에 압축 해제한 뒤 설치 상태를 전환합니다. 다운로드 취소나 검증 실패에서는 현재 설치를 보존합니다. Inno Setup은 프로그램의 설치 파일이 직접 설치하므로 설치 중단이나 프로그램별 쓰기 실패는 자동 복구되지 않을 수 있습니다. 두 형식 모두 업데이트 전에 대상 프로그램이 실행 중인지 확인합니다. Moa Launcher 자체 업데이트는 아직 지원하지 않습니다.

ZIP 앱은 사용자 설정을 AppData 같은 별도 위치에 저장하는 구성을 권장합니다. 실행 파일 옆에 저장한 설정은 이전 버전 폴더에 남으며 새 버전으로 자동 복사하지 않습니다. Inno 설치가 시작되면 취소는 설치 프로그램 창에서 진행합니다.

## 설계와 단계 기록

런처는 .NET 8 WPF 한글 앱입니다. 코어 라이브러리가 카탈로그, GitHub 릴리스, 체크섬, 다운로드 및 설치 규칙을 보유하며 앱 화면은 이를 호출합니다. [설계와 구성](docs/architecture.md) 및 [개발 단계 기록](docs/development-log.md)을 참고하세요.

[Craft Launcher](https://github.com/storytold/craft-launcher)의 공개 앱 목록 및 릴리스 기반 관리 흐름을 참고했습니다. 앱의 코드, 로고, 이미지 자산은 복사하지 않은 독립 구현입니다.
