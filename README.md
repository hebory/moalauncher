# Moa Launcher

GitHub Releases에 배포한 모아 프로그램을 한곳에서 설치하고 업데이트하는 Windows 런처입니다.

## 개발 단계

1. 기본 프로젝트와 확장 가능한 앱 카탈로그
2. GitHub 릴리스 조회와 검증된 설치·업데이트
3. 한글 데스크톱 화면과 앱 관리
4. 테스트, 실행 검증, 배포 패키지

각 단계는 한글 메시지의 Git 커밋으로 기록합니다. C# / .NET 8 / WPF를 사용하며 화면과 설치 엔진을 분리합니다.

## 참고

[Craft Launcher](https://github.com/storytold/craft-launcher)의 앱 카탈로그와 설치·업데이트 흐름을 참고한 독립 구현입니다. 해당 프로젝트의 코드, 로고, 이미지 자산은 포함하지 않습니다.

모아뷰의 저장소 주소가 확인되면 `catalog/apps.json`의 `repository`에 `owner/repo` 형식으로 연결합니다. 현재 항목은 로컬 모아뷰 프로젝트에서 확인한 Inno Setup 배포 규칙을 사용합니다.
