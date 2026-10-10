# Совместимость скриптов, импорт зависимостей и Niri

Дата: 2026-10-10. Сборки, fmt/Clippy, Rust-тесты, запуск скриптов, сборка системного
релиза и GUI выполнялись только на выделенных VM. Локально редактировались
исходники/документация и сравнивались SHA файлов. Intel N100 не изменялся.

## Контракт

- `apps run-script` запускает Bash от обычного пользователя Arch/Distrobox.
  HOME и рабочий каталог сохраняются; bare pacman получает контейнерный sudo
  через дочерний PATH. `makepkg` остаётся непривилегированным. Root всего скрипта
  требует явного `--root`. Контейнерная обёртка заменяется атомарно.
- `apps export-packages` экспортирует все явные запросы контейнера, разделяя
  repository packages и foreign/AUR. `packages_from` подключает просмотренный
  список к base.yaml. Системный resolver использует только официальный Arch
  core/extra, подписанные архивы и выбранную дату; транзитивные зависимости
  определяются автоматически. Непустой foreign блокирует импорт.
- `desktop.environment: niri` предоставляет отдельный профиль; `environment:
  plasma` + `sessions: [niri]` сохраняет KDE и добавляет Niri/Quickshell. SDDM,
  graphical.target и health checks учитывают оба варианта.
- Frozen build/installer сохраняет раскрытые package sets без внешних include.
  В обнаруженном дефекте относительный `base.lock` записывался, но fsync каталога
  возвращал ENOENT. Исправлен пустой parent в `util::atomic`, полный VM-прогон
  повторён; реальная команда `looom lock base.yaml` затем завершилась успешно.

Интерфейс и правила: [apps.md](../../apps.md).

## Development VM и Rust

`Cachyos`, MAC `52:54:00:7b:23:63`, UEFI, 6 vCPU/8 GiB, vda 60 GiB.
Рабочий подтверждённый релиз `rust-gc-ready` сохранялся. Toolchain:
`@toolchain-20261006`, `/var/lib/looom/dev/root`; исходники
`/var/lib/looom/dev/src`, bind внутри chroot в `/workspace`.

```sh
cd /workspace
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
LOOOM_TEST_VM=1 cargo test --locked -- --test-threads=1
cargo build --locked --release --bin looom
```

Все 11 интеграционных тестов прошли: apps 2, configuration 7, bootstrap 1,
runtime/credentials/publication/GC 1. Новые проверки покрывают слияние package
sets, изменение request, автономный frozen config, дубли/foreign/anchors/
symlink/выход пути, Niri и обязательный SDDM.
[Окончательный прогон](primary/rust-final.log),
[первый прогон](primary/rust-first.log), [SHA исходников](source.sha256).

Финальный бинарник:

```text
d76cdb361bde3f7d9469734fb8f23e98b0b812fd68cb72b5cf2ddd9ce361e9db
```

Локальный артефакт: `.local/artifacts/looom-0.2.0-script-compat-20261010-x86_64`.
Он не включается в Git; GitHub содержит исходники для сборки.

## Desktop VM и скрипты

`Looom-feedback-limine`, MAC `52:54:00:10:10:11`, UEFI/Limine 12.9.3,
4 vCPU/4 GiB, vda 64 GiB, пользователь kexfff UID/GID 1000.
Контроллер: [test-script-compat.py](../../../scripts/test-script-compat.py).
Окончательный прогон использовал установленный `/usr/bin/looom` нового релиза.

- EUID/whoami/HOME внутри скрипта соответствуют обычному пользователю.
  Файл результата принадлежит UID 1000; поддерживающий файл читается из cwd.
- Bare pacman устанавливает figlet/base-devel; sudo с абсолютным pacman
  устанавливает bc. Literal argv с пробелами, `;` и `$()` передаётся без выполнения.
- Реальный `makepkg` собирает `looom-compat-fixture-local`, установка проходит
  контейнерным pacman. Сценарий root отказывает без `--root` и работает с флагом.
- Экспорт разделяет repository requests и локальный foreign пакет. Повторная
  запись существующего файла отказывает, stdout выдаёт тот же документ.
  Host inventory, apps.yaml и родительский PATH сохраняются при запуске скрипта.

[Прогон установленным бинарником](desktop/script-installed.log).

Настоящий iNiR клонирован на VM из upstream, commit
[`48cdcecb0f6089cfbb7019054602eab28e154b3b`](https://github.com/snowarch/iNiR/tree/48cdcecb0f6089cfbb7019054602eab28e154b3b).
`looom apps run-script …/iNiR/setup status` завершается успешно от пользователя;
`--root …/setup status` отказывает собственной проверкой iNiR.
[Окончательный status](desktop/inir-status-final.log),
[root отказ](desktop/inir-root.log), [ревизия](desktop/inir-commit.txt).
Одна промежуточная status-проверка была прервана тестовым reboot и повторена
после возврата; окончательный журнал относится к завершённому запуску.
Полная установка iNiR, его optional AUR themes/plugins и live-сессия всей оболочки
в этом прогоне не заявляются.

## Реальный системный релиз

Из экспорта взяты 34 repository requests; единственный foreign явно оставлен
в контейнере. Имена зависимостей не переносились по одному вручную.
Декларация базового KDE дополнена:

```yaml
desktop:
  environment: plasma
  sessions: [niri]
packages_from:
  - container-packages.yaml
```

В State `/var/lib/looom/dev/compat-20261010` выполнены:

```sh
./looom lock base.yaml
./looom plan base.yaml
./looom build base.yaml compat-niri
./looom publish compat-niri
./looom try compat-niri
systemctl reboot
# После загрузки:
looom verify
looom confirm compat-niri
```

Resolver получил 807 подписанных пакетов snapshot `2026-10-07`.
Нативный `/usr/bin/looom` имеет точный SHA финального бинарника; root read-only,
SDDM и обязательные units активны, пакетный inventory совпадает с frozen lock.
В меню сессий присутствуют `niri.desktop` и `plasma.desktop`.
Версии: Niri 26.04-1, Quickshell 0.3.1-1, Kirigami 6.30.0-1.
Импортированный figlet доступен на хосте; foreign fixture в host root отсутствует.
[Lock](desktop/lock.log), [build](desktop/build.log), [boot](desktop/boot.log),
[подключаемый список](desktop/container-packages.yaml).

## GUI и откат

[test-niri-quickshell.py](../../../scripts/test-niri-quickshell.py) запускается
как session program виртуального KWin. Внутри него работает **нативный Niri**;
через Niri IPC запускается нативный Quickshell с импортом Kirigami. IPC windows
содержит настоящее окно с title `looom Niri Quickshell acceptance`.

```sh
DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/1000/bus \
XDG_CURRENT_DESKTOP=KDE XDG_SESSION_TYPE=wayland LIBGL_ALWAYS_SOFTWARE=1 \
timeout 50 kwin_wayland --virtual --xwayland --no-lockscreen \
  --no-global-shortcuts --socket looom-compat \
  --exit-with-session=/home/kexfff/looom/compat-acceptance/test-niri-quickshell.py
```

[GUI](desktop/gui-niri.log), [Niri session](desktop/niri-session.log),
[окно](desktop/niri-window.json). Software rendering и сообщения недоступного
DRM в виртуальном окружении сохранены в логах; физический GPU и вход в Niri через
SDDM этим тестом не проверяются. Пользовательские конфиги Niri не заменялись.

Пройден цикл `apps-b → compat-niri → apps-b → compat-niri` с реальными reboot.
На предыдущем релизе `verify` проходит; Flatpak Calculator, Arch figlet и AppImage
запускаются. Сравниваются SHA apps.yaml, applied.json, active AppImage и данных
скрипта, ID контейнера и commit Flatpak. После возврата ВМ остаётся на
подтверждённом `compat-niri`; оба предыдущих подтверждённых релиза сохраняются.
[Откат](desktop/rollback.log), [данные при откате](desktop/rollback-user.log),
[возврат](desktop/return.log), [данные при возврате](desktop/return-user.log).

## Публичные доказательства

Логи проверены на каждой исходной VM против локальных credentials до передачи.
Сырые password hashes, private keys, токены и private installer fixtures
не экспортировались. [Primary audit](primary/public-audit.log),
[desktop audit](desktop/public-audit.log).
SHA публикации приведены в `public-artifacts.sha256`.
