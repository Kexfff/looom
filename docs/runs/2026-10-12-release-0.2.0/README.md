# Приёмка looom 0.2.0

Дата: 2026-10-12. Сборка, выполнение продукта и все тесты — **только в VM**.
На хосте выполнялись редактирование, передача публичных файлов, управление
libvirt, проверка контрольных сумм и публикация.

## Артефакты и входы

Сборочный commit: `13eabe8244dbb245278b789fd67ef5749e2f50c7`.
Последующие изменения для выпуска — документация, приёмочные контроллеры
и этот журнал; исходники Rust и входы ISO после сборки не менялись.

| Артефакт | SHA256 |
| --- | --- |
| `looom-0.2.0-x86_64-unknown-linux-gnu` | `d89ecd0dc663df14d52167f3a9c2c39f504f2db97e825ebc2c65ecbed906f17b` |
| `looom-0.2.0-x86_64.iso` | `b4ce836240c108c6854f123bb86684f27bab0b71ef7332a220b14755afc9519d` |

Размер ISO: 1 706 964 992 байта. [SHA256SUMS](SHA256SUMS),
[манифест ISO](looom-0.2.0-build.txt), [live-пакеты](looom-0.2.0-packages.txt),
[SHA исходников Rust](source.sha256), [состояние установленной системы](result.json).
Arch Archive `2026-10-07`, Archiso `91-1`, `SOURCE_DATE_EPOCH=1791763200`.
Установленная база содержит 720 подписанных пакетов с точными версиями:
[декларация и lock initial](initial-base.yaml), [lock](initial-base.lock).
Зафиксированы входы и процесс; побитовая идентичность повторной сборки ISO не заявляется.

## Rust и сборка ISO

VM `Cachyos`, MAC `52:54:00:7b:23:63`, 6 vCPU/8 GiB, VirtIO/Btrfs 60 GiB.
Работающий подтверждённый `rust-gc-ready` оставался read-only. Сборка в сохранённом
toolchain-подтоме, ISO — в отдельном Archiso builder.

```sh
arch-chroot /var/lib/looom/dev/root bash -c '
  cd /workspace &&
  cargo fmt &&
  cargo clippy --locked --all-targets -- -D warnings &&
  LOOOM_TEST_VM=1 cargo test --locked -- --nocapture --test-threads=1 &&
  cargo build --locked --release --bin looom
'
```

Все **12 Rust-тестов** прошли: один library-тест SSH, два apps-теста,
один bootstrap-тест, семь configuration-тестов и один runtime-тест.
Clippy — без предупреждений. Toolchain: [Rust/Cargo 1.99.0, LLVM 23.1.1](toolchain.log).
Внутри интеграционных тестов проверены скрытые
credentials/ACL/конкурентность, bootstrap interruption/recovery, публикация,
защита выбранных и смонтированных релизов и четыре прерванных удаления GC.
[Полный журнал](rust.log).

ISO собран [scripts/build-iso.sh](../../../scripts/build-iso.sh) в отдельном
builder с новым work-каталогом и точным проверенным release-бинарником.
[Журнал](build-iso.log). В payload нет SSH host/private keys, authorized keys,
password hashes, машинного State и истории сборщика; machine-id имеет стандартный
маркер `uninitialized`. Пакетный keyring также не содержит приватных `.key`.
[Аудит payload](iso-payload-audit.log).

## Чистая установка и первый вход

Новая VM `Looom-release-0.2.0`, MAC `52:54:00:20:02:00`, адрес
`192.168.122.156`, 4 vCPU/8 GiB, VirtIO 64 GiB, UEFI/OVMF с отдельной NVRAM,
Secure Boot выключен. Старые установленные виртуальные диски не форматировались.

Контроллер [test-release-iso.py](../../../scripts/test-release-iso.py)
проверяет конкретный MAC, виртуализацию, UEFI, размер диска и точный SHA менеджера.
На реально загруженном ISO проверены декларации, запуск установщика из меню,
номер диска, исключение loop/CD-ROM и отмена `0` с сохранением первого MiB диска.

Полная установка прошла настоящий PTY-мастер: диск `1`, hostname `looom-020`,
пользователь `kexfff`, только KDE, `YES`, скрытые root/user passwords.
Для ускорения использованы 718 заранее проверенных **публичных** пакетов и
подписей из SDK VM; установщик самостоятельно проверил итоговые metadata,
хэши и подписи, скачал оставшиеся входы. Это приёмка с тёплым кэшем, не замер
холодной загрузки. Приватные ключи и credentials между VM не переносились.
Управляющий публичный SSH-ключ добавлен явно только для теста; ISO его не содержит.

После `Installation complete` ISO извлечён. Реальная загрузка — Limine,
`/@root-initial`, `ro`, работающие SDDM/питание, точные inventory/manager,
editable `~/looom/{base.yaml,base.lock,apps.yaml}` с UID/GID пользователя.
[Установка](install.log), [проверка первого boot](boot.log), [SDDM](sddm.png).

В KDE действительно выполнен вход через SDDM. Один раз открылся Konsole
с `looom setup`; marker принадлежит пользователю и имеет `0600`.
[Первый вход](first-login.png). `setup --status` и вызов без TTY не подтверждают
систему и не применяют apps. Выбор `2` прошёл настоящий sudo prompt, verify
и confirm. Выбор `1` с `YES` установил пользовательский Firefox.
[Setup](setup.log), [подтверждение](setup-confirm.log), [apps](setup-apps.log).
Firefox действительно открыл окно в Plasma Wayland: [скриншот](firefox.png).

## Приложения, загрузки, откат и GC

[Контроллер](../../../scripts/test-release-desktop.py) выполнялся в VM от root,
а действия apps/setup — через обычного пользователя с активной сессией.

1. AppImage `appimagetool` из официального release 1.9.1 проверен по SHA256
   и запущен через looom launcher. Rootless Arch/Distrobox создан, `tree`
   установлен декларативно. Реальный `.sh` с `pacman`, `sudo /usr/bin/pacman`,
   проверкой container/root и буквальными аргументами выполнился в контейнере;
   `jq`/`bc` запустились, host остался read-only. [Журнал](apps.log).
2. `rebuild … desktop-next` выполнил все пять стадий; реальный trial boot
   сохранил apps/пароль, следующий reboot **без confirm** вернул `initial`.
   [Rebuild](rebuild.log), [trial](trial.log), [fallback](fallback.log),
   [apps после fallback](fallback-apps.log).
3. Исправленная тестовая декларация добавила `/etc/looom-release-proof.conf`:
   [YAML](configured/base.yaml), [lock](configured/base.lock), [файл](configured/proof.conf).
   Собран и загружен `desktop-changed`; файл существует, точные IDs/commit,
   содержимое apps/HOME/local settings и credentials совпали с исходным private baseline.
   Релиз подтверждён. [Сборка](rebuild-configured.log), [приёмка](trial-configured.log).
4. Ручной rollback на `initial` и настоящий reboot убрали декларативный файл.
   Пароль, Flatpak commit, контейнер, AppImage, HOME и `/etc/looom-local` сохранились.
   [Журнал](rollback.log). Значения credentials сравнивались только внутри VM.
5. GC preview/apply удалил отвергнутый `desktop-next`, его read-only subvolume
   и UKI. Работающий `initial` и ожидающий trial `desktop-changed` сохранились.
   [Журнал](gc.log).
6. Меню `setup` изменило пароль управляемого пользователя: скрытые sudo,
   new/repeat prompts, runtime synchronized. Затем возврат на `desktop-changed`,
   reboot и настоящий вход KDE **новым паролем**. [Проверка](password.log),
   [PTY](setup-password.log).
7. Финальная проверка подтвердила каждую audio/power/portal-службу отдельно,
   повторный `--first-login` не открыл мастер; выбор `5` с `YES` выполнил
   обновление объявленных приложений. [Итог](final.log), [update](setup-update.log),
   [состояние](status.log), [финальный Firefox](final-desktop.png).

При разработке контроллера первая проверка rootless apps была запущена до
пользовательского входа, а первоначальная текстовая правка YAML не учитывала
JSON-экспорт мастера. Это ошибки приёмочного сценария: добавлены явная проверка
активной сессии и структурная правка декларации. Исходный private baseline
не заменялся для обхода несовпадений. Диагностика сохранена:
[до входа](trial-before-login.log), [первый fixture](trial-fixture-diagnostic.log),
[snapshot без сессии](snapshot-session-diagnostic.log).
Также усилена проверка systemd: многократный `is-active` возвращает успех при
хотя бы одном активном unit; контроллер теперь сравнивает каждую строку.

## Публичность и ограничения

Логи и изображения проверены **на исходных VM** против credential hashes,
старого и нового private login fixture; private State, backups и пароли не
экспортировались. [Аудит SDK](public-audit.log), [аудит desktop](desktop-public-audit.log),
[контроллер аудита](../../../scripts/audit-release-public.py).

Это UEFI/KVM-приёмка нового выпуска. Нет Bluetooth/Wi-Fi адаптера в VM:
Bluetooth включён, но радиосвязь не проверена. Проверены службы PipeWire,
не физический звук/микрофон. Suspend/resume, NVIDIA, Secure Boot, шифрование,
dual boot и новая установка этого ISO на N100 не проверялись.
Предыдущий физический пилот имеет [отдельный журнал](../2026-10-07-n100/README.md).
Новый выпуск пригоден для согласованного повседневного тестирования в заявленном
профиле; это не обещание поддержки всех компьютеров или отсутствия ошибок.
