# 앱 카탈로그

런처 시작 시 앱에 포함된 `catalog/apps.json`을 읽습니다. 사용자 정의 목록은 `%LOCALAPPDATA%\MoaLauncher\catalog.json`에 저장합니다. 사용자 항목은 같은 ID를 가진 기본 항목을 덮어쓰고, 기본 목록에 없는 앱은 추가합니다.

## 앱 추가 대화상자

프로그램 카드의 **··· → 프로그램 설정** 또는 **프로그램 추가**를 선택하고 표시 이름, GitHub 저장소, 배포 파일, 실행 파일을 입력합니다. ZIP 앱은 압축 파일 안의 실행 파일 경로를 기준으로 관리됩니다. Inno Setup 앱은 설치 실행 파일명 패턴과 `%LOCALAPPDATA%` 기준 설치 폴더를 입력합니다.

앱 설정은 사용자 카탈로그에 저장되며 사용자 계정에서 읽고 쓸 수 있습니다. 프로세서별 패키지 형식을 지원하며 현재 Windows 환경에 해당하는 패키지만 선택합니다.

## JSON 형식

새 앱의 가장 작은 등록 예시입니다.

```json
{
  "schemaVersion": 1,
  "apps": [
    {
      "id": "moaview",
      "name": "모아뷰",
      "description": "파일을 미리 보는 도구",
      "category": "파일 · 미리보기",
      "accentColor": "#8BA3FF",
      "monogram": "Mv",
      "repository": "hebory/moaview",
      "packages": [
        {
          "architecture": "x64",
          "kind": "inno",
          "assetPattern": "MoaView-*-Setup-x64.exe",
          "executablePath": "MoaView.exe",
          "installDirectory": "Programs/MoaView"
        }
      ]
    }
  ]
}
```

기본 모아뷰 항목은 사용자가 확인한 `hebory/moaview` 저장소에 연결됩니다. 다른 프로그램을 등록할 때는 해당 프로그램의 실제 `계정명/저장소명`을 입력하세요. 빈 `repository`도 표시용 항목으로 읽을 수 있지만 릴리스를 조회하거나 설치할 수 없습니다.

`id`는 소문자 영문, 숫자, 하이픈으로 구성하며 앱마다 고유합니다. 카탈로그는 최대 100개 앱을 받습니다. `architecture`는 `x64`, `arm64`, `x86` 중 하나입니다. 앱에 여러 CPU별 패키지를 둘 수 있습니다. `kind`는 `zip` 또는 `inno`입니다. `assetPattern`은 파일명만 검사하고 `*`, `?`를 지원하며 `/`, `\`, `..`를 허용하지 않습니다. 각 패턴은 릴리스에서 정확히 하나의 파일만 찾아야 합니다. `executablePath`는 ZIP 또는 지정된 설치 폴더를 기준으로 한 상대 경로여야 합니다. `installDirectory`는 Inno Setup 패키지에만 쓰며 LocalAppData를 벗어날 수 없습니다.

## GitHub Releases 요건

`repository`는 공개 GitHub 저장소여야 합니다. 런처는 `releases/latest` API를 사용하며 draft와 prerelease는 설치 대상으로 삼지 않습니다. API에 등록된 파일 중 이름이 `assetPattern`에 맞는 배포 파일을 선택합니다. `browser_download_url`는 HTTPS와 해당 저장소의 release-download 주소를 사용해야 합니다. 임의 URL, 다른 저장소의 파일, 이름이 모호하게 겹치는 파일은 거부합니다.

릴리스 태그는 `v1.2.3` 또는 `1.2.3`처럼 Semantic Versioning을 사용하세요. 버전 비교가 가능한 태그여야 업데이트를 표시합니다. Inno Setup의 실행 파일 ProductVersion도 같은 버전이어야 하며, 설치기는 `/SILENT /DIR=... /NORESTART` 실행을 지원해야 합니다. 무인 설치에서 건너뛰는 필수 구성 요소가 있다면 앱 배포자가 설치기에 포함하거나 별도로 안내해야 합니다.

다운로드는 자산 API `digest`의 SHA-256, 자산명에 `.sha256`을 붙인 체크섬 파일, 또는 `SHA256SUMS.txt`에서 해당 파일 이름으로 식별한 해시를 확인해야 합니다. 체크섬 파일이 없거나 파일과 연결된 해시가 없으면 설치를 중단합니다. ZIP은 임시 폴더에 안전하게 풀고 실행 파일을 확인합니다. 검증과 압축 해제에 실패하면 현재 설치 상태를 유지합니다. Inno Setup은 기존 설치를 실행해 자체적으로 파일을 변경하므로 설치 도중 실패한 파일 변경은 자동으로 되돌릴 수 없습니다.

## 카탈로그 가져오기와 내보내기

설정에서 공개 JSON 파일을 가져올 수 있습니다. 파일을 선택하면 런처가 앱 이름과 저장소 목록을 보여 주고 승인을 받은 항목을 병합합니다. JSON 앱 카탈로그를 신뢰할 수 있는 게시자의 GitHub Release에서 배포하세요. JSON을 가져오는 것만으로 앱이 자동 다운로드되거나 설치되지는 않습니다. 내보내기는 현재 화면에 표시하는 기본 앱과 사용자 앱을 JSON 파일로 저장합니다.
