# Read-only релизы в прототипе VM

Историческая инструкция Python/Bash-прототипа 2026-10-04.
Текущий менеджер полностью на Rust: [контракт и команды](native-rust.md),
[новый журнал](runs/2026-10-06-native-rust/README.md).

Сценарии предназначены для одной тестовой VM из
[bootstrap-инструкции](bootstrap-installation.md). Диск повторно не форматируется.
Зафиксированный источник: Arch Archive 2026/10/03. KDE добавляется только после
успешного теста двух консольных релизов и отката.

Результаты текущего запуска: [журнал](runs/2026-10-04-releases/README.md).

Проверено 2026-10-04: v1r3 -> v2 -> v1r3 с разными ядрами;
прерывание сборки и публикации; возврат после неподтверждённого неисправного старта;
реальная Plasma Wayland и desktop -> v1r3 -> desktop. VM оставлена в desktop,
исходный пароль пользователя восстановлен, временное тестовое состояние удалено.
Package locks и метаданные: `manifests/releases/`.

## Доступ и подготовка

С хоста проекта:

```bash
scripts/vm-ssh.sh 'findmnt -nro FSROOT /; systemctl --failed'
tar -cf - scripts configs docs/runs/2026-10-04-bootstrap/evidence/mkinitcpio.conf \
  | scripts/vm-ssh.sh 'mkdir -p /var/lib/looom/release-source && tar -xf - -C /var/lib/looom/release-source'
scripts/vm-ssh.sh 'python /var/lib/looom/release-source/scripts/initialize-release-state.py'
```

Инициализация выполняется в writable bootstrap и сохраняет существующие password
hashes, SSH identity, machine-id и подключения NetworkManager в закрытом `@state`.
Эти данные не нужно и нельзя переносить в Git, журнал сборки или публикуемый образ.

## Сборка, публикация и пробная загрузка

На VM от root, сначала для профиля `v1`, затем `v2`:

```bash
cd /var/lib/looom/release-source
bash scripts/build-release.sh v1
python scripts/looom-release.py publish v1r3
python scripts/looom-release.py try v1r3
systemctl reboot
```

ID исправленного первого релиза — `v1r3`: исходные `v1` и `v1r1` сохраняются отвергнутыми
после выявленных дефектов раннего NSS и прав каталога `/etc`. `v1r2` исправил их,
но потребовал корректировки пути root SSH; в окончательном v1r3 это также исправлено.
Профиль `v1` и ID релиза различаются намеренно; опубликованный ID не переиспользуется.

После реальной загрузки:

```bash
/usr/lib/looom/verify-release.sh v1r3
looom-release confirm v1r3
looom-release status
```

В `v2` пакет tree и другая декларативная настройка. Ядро меняется с linux
на linux-lts; оба берутся из одного архива. Назначение `try` не подтверждает релиз.
Если проверки не пройдены, следующая перезагрузка загружает прежний постоянный выбор.
Bootstrap остаётся отдельной аварийной записью в меню GRUB.

Корень `/` защищён одновременно mount `ro` и Btrfs property `ro=true`.
`/home`, `/var`, `/var/lib/looom` сохраняются при выборе любого корня.
База пакетов `/usr/lib/looom/pacman` находится внутри релиза.

## Учётные записи и локальные настройки

`/etc/passwd` и `/etc/group` — обычные read-only файлы, доступные уже PID 1,
udev и ранним сокетам. Их UID/GID проверяются по общей карте назначений.
`/etc/shadow` и `/etc/gshadow` ссылаются в `/run/looom/accounts`.
Обязательная ранняя служба читает шаблоны с заблокированными паролями из релиза
и credential-файлы из `@state`, затем создаёт runtime shadow до запуска PAM-потребителей.

Смена пароля:

```bash
sudo looom-password codex
```

Пароль запрашивается интерактивно; в аргументы команд и историю он не попадает.
Стандартные `passwd`, `usermod` и изменение пароля в KDE пока не поддерживаются.
Временный PAM-тест умеет поменять пароль, проверить его после отката и восстановить
исходный, не выводя секретных значений:

```bash
python /var/lib/looom/release-source/scripts/test-persistent-password.py prepare
# после отката:
python /var/lib/looom/release-source/scripts/test-persistent-password.py verify
python /var/lib/looom/release-source/scripts/test-persistent-password.py restore
```

Явные исключения:

| Путь | Источник | Изменение при откате |
| --- | --- | --- |
| `/etc/looom-local` | `@state/local-etc/looom-local` | Сохраняется |
| `/etc/NetworkManager/system-connections` | `@state/local-etc/NetworkManager/system-connections` | Сохраняется |
| `/etc/resolv.conf` | Runtime `/run/NetworkManager/resolv.conf` | Генерируется заново |
| `/etc/looom/declarative-value` | Файл выбранного корня | Откатывается |

Произвольная запись в остальные пути `/etc` отклоняется. Локальные каталоги
подключаются целиком; декларативные файлы внутри них не размещаются.

## Подтверждение и откат

Подтверждение проверяет именно работающий релиз: ID, ядро, монтирования,
наличие загрузочного артефакта, базовые службы и отсутствие failed units.
Для рабочего стола дополнительно нужно проверить реальную графическую сессию.
Общее состояние сервисов в `/var` не откатывается.

```bash
looom-release rollback v1r3
systemctl reboot
# на v1r3:
/usr/lib/looom/verify-release.sh v1r3
```

Откат разрешён только к ранее подтверждённому релизу. ID записи GRUB устойчивый,
например `looom-v1r3`; порядковый номер пункта не используется.

## Прерывания и восстановление

Сборка оставляет приватный `@build-ID`, если завершается ошибкой; меню остаётся прежним.
После успешной сборки read-only `@root-ID` имеет статус validated и ещё не загрузочный.
Публикация сохраняет UKI, синхронизирует ESP, проверяет его SHA-256, затем заменяет меню
и записывает published metadata. Прерывание между шагами допускает повтор публикации.

```bash
looom-release status
looom-release recover
```

`recover` пересобирает меню только из опубликованных релизов и сообщает о незавершённых
операциях. Неудалённые build-корни сохраняются для диагностики. Для fault injection
предусмотрены `LOOOM_FAIL_AFTER=build` и `LOOOM_FAIL_AFTER=uki`.

Для полностью подготовленного приватного корня можно отдельно повторить финализацию:

```bash
bash /var/lib/looom/release-source/scripts/finalize-release.sh desktop
```

Finalizer требует готовый lock, совпадающий фактический список пакетов, locked
шаблоны shadow и прежний SHA-256 UKI. Он отказывается заменять опубликованный ID.
Произвольную незавершённую пакетную установку такой командой финализировать нельзя.
Команда `reject ID` убирает неподтверждённый кандидат из меню, оставляя его корень;
работающий, подтверждённый или назначенный к следующему старту релиз защищён.

На ESP сохраняются `grub.cfg.previous`, UKI bootstrap и EFI fallback.
Если испорчено само меню, из GRUB command line можно загрузить bootstrap:

```text
search --no-floppy --fs-uuid --set=esp 8CE8-BFE9
chainloader ($esp)/EFI/Linux/looom-bootstrap.efi
boot
```

UUID выше принадлежит текущей VM; при новой установке взять значение `blkid /dev/vda1`.
При повреждении ESP целиком потребуется Arch ISO и восстановление загрузчика.
Проверки процесса публикации не заменяют тесты потери питания на физическом накопителе.

## KDE Plasma

После успешного отката:

```bash
cd /var/lib/looom/release-source
bash scripts/build-release.sh desktop
python scripts/looom-release.py publish desktop
python scripts/looom-release.py try desktop
systemctl reboot
```

Состав профиля: минимальный полноценный Plasma Wayland, SDDM, Dolphin, Konsole,
сеть, звук PipeWire/WirePlumber, portal KDE, шрифты и SPICE agent.
Greeter SDDM работает через X11; пользовательская сессия — Wayland.
Графическая сессия не подтверждает релиз автоматически.

После графического входа:

```bash
bash /var/lib/looom/release-source/scripts/verify-desktop.sh
looom-release confirm desktop
```

Проверяются локальная активная Wayland-сессия codex, KWin, Plasmashell,
PipeWire/Pulse/WirePlumber и отсутствие failed user units. Стандартная GUI смена
пароля пока не подключена к credentials; использовать `sudo looom-password codex`.
В актуальном сборщике root `.ssh` подключается отдельным bind mount из `@state`;
старые консольные релизы используют ссылку в это же постоянное хранилище.
NetworkManager имеет `rc-manager=unmanaged`, а DNS генерирует в `/run`.
