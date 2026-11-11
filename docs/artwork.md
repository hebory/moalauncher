# Moa Launcher 이미지

- `icon.svg`: 기존 앱 헤더의 둥근 흰색 m 심볼을 확장한 벡터 원본입니다. 보라색 그라데이션과 투명한 바깥 모서리를 사용합니다.
- `icon.png`: 목록·상세 화면 및 외부 카탈로그용 512×512 PNG입니다.
- `src/MoaLauncher.App/Assets/launcher.ico`: 실행 파일·설치 프로그램·트레이용 아이콘입니다. 16, 24, 32, 48, 64, 128, 256픽셀 프레임을 포함합니다.
- `cover.png`: 상세 화면 및 저장소 소개용 1672×941 PNG입니다. 여러 앱을 한곳에 모아 업데이트한다는 의미의 타일 구성입니다.

앱은 PNG를 WPF 리소스로 포함하므로 첫 실행이나 오프라인에서도 런처 이미지가 표시됩니다. 외부 카탈로그에는 다음 공개 주소를 사용할 수 있습니다.

- https://raw.githubusercontent.com/hebory/moalauncher/master/icon.png
- https://raw.githubusercontent.com/hebory/moalauncher/master/cover.png

아이콘 PNG와 ICO는 Windows에서 벡터 원본으로 다시 생성합니다.

```powershell
powershell.exe -NoProfile -STA -File scripts/Render-LauncherIcon.ps1
```

## 커버 제작

2026-11-11에 내장 `image_gen` 도구로 생성했습니다. 별도 CLI/API 생성 경로는 사용하지 않았습니다. 선택한 결과를 크기 변경이나 합성 없이 `cover.png`에 저장했습니다. 요청 크기는 1536×864였으며 실제 반환 이미지의 크기는 1672×941입니다.

사용한 최종 프롬프트:

```text
Use case: ads-marketing. Asset type: final cover image for the Windows app Moa Launcher, to appear above release details inside a clean light desktop app. Create a polished wide 16:9 cover, 1536x864 pixels. Composition: generous negative space, on the left the exact words "Moa Launcher" in a beautifully spaced bold dark-indigo sans-serif, readable even in a small 500px wide panel. On the right, a refined editorial 3D arrangement of three softly rounded application tiles converging into one larger indigo-violet launcher tile, suggesting collecting apps and keeping them updated. The main tile has a thick white monoline lowercase m formed from two equal rounded arches with three vertical stems, rounded line caps, centered and simple. Small supporting tiles have subtle minimal document/window and update-arrow symbols, no letters. Palette: near-white lavender background, indigo #5369E8, periwinkle #8BA3FF, a tiny mint accent. Soft studio lighting, satin surfaces, gentle ambient shadows, very restrained glass-like detail. Clean balanced geometry, premium friendly productivity software, no busy tech circuits, no cartoon rocket, no photo, no mock window frame, no extra text, no watermarks. The full cover should look intentional and elegant, uncluttered.
```
