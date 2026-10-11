# iNiR на looom

Профиль устанавливает пользовательскую оболочку [iNiR](https://github.com/snowarch/iNiR)
2.33.0 на штатный Niri. Upstream зафиксирован в `upstream-revision.txt`;
версия Python — 3.12.15, зависимости — в `python-requirements.lock`.
Системные пакеты берутся из официального Arch snapshot вашей декларации.
Проверенный snapshot — `2026-10-07`; при смене даты меняются системные версии.
KDE можно сохранить как запасную сессию.

## Установка

Нужен установленный looom с командой `rebuild` и доступом к интернету.
Выполняйте пользовательские шаги из обычной учётной записи.
На VM включите **Virtio → 3D acceleration** и **SPICE → OpenGL**.
Если XML явно задаёт `device='virtio-vga'`, уберите этот атрибут: итоговая
модель должна быть `virtio-vga-gl`. Niri требует 3D в виртуальной машине:
[официальная инструкция](https://niri-wm.github.io/niri/Getting-Started.html).

```sh
git clone https://github.com/Kexfff/looom.git ~/looom-src
cd ~/looom-src
bash examples/inir/install.sh prepare ~/looom/base.yaml
sudo looom rebuild ~/looom/inir/declaration/base.yaml inir-desktop
sudo reboot
```

Если на VM экран входа Breeze показывает только фон, используйте при подготовке:

```sh
bash examples/inir/install.sh prepare ~/looom/base.yaml --software-greeter
```

Это добавляет в декларацию программную отрисовку **только greeter SDDM**;
Niri продолжает использовать 3D. Такой вариант проверен на тестовой VM.
Выберите нужный вариант `prepare` до сборки. Повторный `prepare` не перезаписывает
существующий каталог декларации. Для другого независимого опыта можно задать
`LOOOM_INIR_DIR=~/looom/inir-other` перед обоими вызовами скрипта.

После загрузки нового релиза войдите в KDE или откройте SSH/TTY от своего пользователя:

```sh
cd ~/looom-src
bash examples/inir/install.sh install
sudo looom verify
sudo looom confirm inir-desktop
```

Выйдите из текущей графической сессии и выберите **Niri** в SDDM.
Для будущих системных пересборок используйте
`~/looom/inir/declaration/base.yaml` и новый идентификатор релиза.
Его `base.lock` лежит в отдельном каталоге. Исходная декларация, её lock,
аккаунты, имена password secrets и уже подключённые списки пакетов сохраняются.
Скрипт подготовки читает JSON-формат YAML, который создаёт установщик looom,
и переносит подключённые пакетные списки и `files.source` в новый каталог.
Для YAML в обычной записи перенесите декларацию в отдельный каталог со всеми
её файлами, подключите `packages.yaml` через `packages_from`, добавьте сессию
`desktop.sessions: [niri]` и включите `bluetooth.service`/
`power-profiles-daemon.service`; затем выполните ту же сборку и файловую установку.

## Где находятся настройки

- `~/.config/inir/config.json` — пользовательские настройки оболочки.
- `~/.config/niri/config.kdl` и `config.d/` — композитор и сочетания клавиш.
- `~/.config/quickshell/inir/` — установленная оболочка и её scripts/QML.
- `~/.config/systemd/user/inir.service` — автозапуск только в сессии Niri.
- `~/looom/inir/backups/` — локальные резервные копии до установки.
- `config-overrides.json` в этом примере — переносимые настройки проверенного
  профиля, без паролей, keyring, сетевых подключений и личных данных.

`install` применяет небольшие настройки поверх upstream: Noto Sans,
Papirus, режим с меньшим количеством эффектов и Breeze вместо необязательного
AUR-плагина Darkly. Изменяйте их в `config-overrides.json` перед установкой
либо через `inir settings`. Повторный `install` снова применяет эти значения.
Файловая установка также меняет пользовательские настройки терминалов,
GTK/KDE и shell profiles; сначала создаётся частная локальная резервная копия.
Не публикуйте каталог backups: он может содержать ваши личные данные.

Сочетания клавиш: **Super+Space** — обзор/приложения, **Super+,** — настройки,
**Super+V** — буфер обмена, **Super+Return** — терминал, **Super+/** — подсказка.
Диагностика: `systemctl --user status inir`, `journalctl --user -u inir`,
`inir settings`, `inir restart`.

## Совместимость

`looom-readonly.patch` отключает установку системной темы SDDM, обычную проверку
host pacman-обновлений и изменяющие host-пакеты действия меню iNiR.
Систему обновляйте через looom, приложения — через Flatpak/Distrobox/AppImage.
Пакетные действия поиска отключены в профиле. Автообновление самого iNiR тоже
отключено, чтобы оно не заменило закреплённый source и правки.
Не запускайте upstream `setup install` целиком с sudo или через container,
ожидая, что контейнер установит хостовую сессию.

Список `packages.yaml` получен из проверенных upstream Arch PKGBUILD-групп,
с добавлением `matugen`, `lsp-plugins-lv2`, инструментов сборки Python-расширений,
сетевого редактора и power profiles. Транзитивные зависимости разрешает looom.
В профиле нет автоматического импорта AUR: дополнительные темы Darkly/WhiteSur,
шрифты и редактор эффектов Gowall не установлены. Их отсутствие не мешает
проверенной основной оболочке. `inir doctor` может предлагать установку этих
опциональных компонентов; не применяйте его исправления системных пакетов на хосте.

Проверены настоящая SDDM/Niri-сессия, панель, обзор, настройки, clipboard watcher,
уведомления, терминал и перезапуск оболочки. VM не подтверждает работоспособность
Bluetooth, яркости монитора, отпечатков, аппаратных power profiles, AI-ключей и
сетевых интеграций. Пользовательские настройки iNiR остаются в HOME и переживают
откат системного релиза; резервную копию HOME нужно восстанавливать отдельно.

Журнал проверки: [2026-10-11-inir](../../docs/runs/2026-10-11-inir/README.md).
