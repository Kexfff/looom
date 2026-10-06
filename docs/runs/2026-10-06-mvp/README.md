# Rust/YAML MVP и проверки отказов, 2026-10-06

Все тесты, сборки, форматирование Rust и compiler checks выполнялись в VM.
Хост использовался для редактирования/передачи файлов, просмотра VM и libvirt reset.
Машина та же: `192.168.122.91`, MAC `52:54:00:7b:23:63`, UEFI/GRUB/Btrfs.
Пароли и хэши managed users не включаются в этот журнал и evidence.

## Исходное состояние

Запущен подтверждённый desktop-релиз от 2026-10-04. Обнаружен failed shadow.service:
`pwck`/`grpck` открывали симлинки `/etc/shadow` и `/etc/gshadow` с O_NOFOLLOW,
получая ELOOP. Причина подтверждена strace в VM.

## Изменения

- Усилен credentials-компонент: проверка владельцев/типов/прав/ACL,
  no-follow доступ через дескрипторы, общий lock, yescrypt, журналируемая смена
  и восстановление runtime из сохранённого хэша.
- shadow.service проверяет runtime-файлы явно после генератора аккаунтов.
- Добавлен Rust CLI и строгая схема. YAML anchors/tags/aliases/merge,
  дубли ключей, неизвестные поля, конфликтующие пути и источники-симлинки отвергаются.
- Пакетный lock создаётся до сборки; pacstrap устанавливает конкретные архивы
  через -U. Архивы, подписи, базы репозиториев и установленное замыкание проверяются.
- Рецепты, конфигурация, байты файлов и CLI замораживаются перед сборкой.
- Восстановление завершает configured private build без автоматической публикации.

## Проверки и результаты

| Проверка | Результат |
| --- | --- |
| Credentials: чтение непривилегированным пользователем, неверные права/владелец/ACL/симлинк/формат | PASS, отдельные fixtures |
| Credentials: 8 конкурентных изменений и прерывание после commit | PASS, актуальное значение соответствует runtime после восстановления |
| Rust: положительная схема и 17 отрицательных вариантов плюс источник-симлинк | PASS, 3 integration tests |
| Cargo clippy all-targets с запретом warnings | PASS |
| Bash/Python syntax checks, три YAML-примера, ссылки и code fences руководства | PASS, только в VM |
| Настоящий FAT ENOSPC при публикации UKI | PASS, меню/постоянный выбор не изменились |
| Прерывание после durable UKI, recover и повторная публикация | PASS, незарегистрированный кандидат не включён в меню |
| Устаревший lock и повреждённый SHA архива | PASS, отказ до создания build/metadata |
| yescrypt в библиотеках v1r3/v2/desktop/mvp-a4/mvp-b | PASS, без настоящих пользовательских паролей |
| mvp-a4: read-only загрузка, ключевой SSH, mounts, пакетная база, shadow.service | PASS, подтверждён установленным CLI |
| mvp-b: fail-after-build и recover | PASS, до recover нет root/metadata; после него validated и явная публикация |
| mvp-a4 -> mvp-b -> mvp-a4 | PASS, реальное переключение linux/linux-lts и возврат |
| Удаление tree, кастомного конфига и mask bluetooth в B; возврат при rollback | PASS |
| Новый пароль через PAM, /home UID/GID и локальный маркер после переключения и rollback | PASS |
| mvp-desktop: сборка установленным looom через собственные замороженные рецепты | PASS, 651 пакет, без development backend override |
| mvp-desktop: read-only загрузка, SDDM PAM, KDE Wayland, PipeWire/WirePlumber | PASS, релиз подтверждён |
| Восстановление исходного пароля после тестов | PASS, временный пароль и его backup удалены из protected test directory |

## Итоговое состояние VM

Подтверждён `mvp-desktop`, GRUB saved_entry — `looom-mvp-desktop`, next_entry отсутствует.
Корень `/@root-mvp-desktop` read-only, ядро `7.2.8-arch1-2`, KDE Plasma `6.7.5-1`.
Реальная локальная Wayland-сессия codex активна; оболочка, KWin и аудиослужбы проверены.
План для исходной desktop-декларации не содержит изменений пакетов, файлов и units.
Исходный пароль восстановлен; `/var/lib/looom/private-password-test` удалён.
Проверенные console-релизы и предыдущие релизы сохранены для ручного отката.

Новые console-релизы: mvp-a4 — 210 пакетов, linux `7.2.8-arch1-2`;
mvp-b — 209 пакетов, linux-lts `6.18.54-2-lts`.
Для desktop используются тот же Arch snapshot `2026-10-03` и явный модуль Plasma.

## Промежуточные отказы

`mvp-a` и `mvp-a2` остановились в private build на проверке enabled для static units
qemu-guest-agent.service и shadow.timer. Эти units теперь учитываются через
их реальную device/vendor активацию. `mvp-a3` остановился при sshd -t без рабочих
host keys; проверка теперь использует отдельный временный ключ и удаляет его.
Меню и постоянный выбор во всех случаях оставались прежними.

Первый reboot после рабочего сеанса имел длинную остановку. Внешний reset
вернул VM на прежний saved desktop при уже очищенном one-shot.
Повторная пробная загрузка mvp-a4 с подключённой serial console прошла успешно.
Это не считается проверкой всех power-cut границ загрузчика.

## Воспроизведение

[Руководство](../../declarative-mvp.md),
[console A](../../../configs/mvp/base.yaml),
[console B](../../../configs/mvp/b/base.yaml),
[desktop](../../../configs/mvp/desktop/base.yaml).

В VM остаются `/var/lib/looom/inputs/<id>` с замороженными рецептами/входами,
`release-evidence/<id>` с пакетными отчётами и `/var/lib/looom/dev/evidence`
с результатами тестов. Локальная копия evidence создаётся без credentials.
Архивы пакетов/подписи и базы репозиториев остаются в кэшах VM; в Git сохраняются
входные lock, отчёты, исходники и журналы. Для сборки требуется этот кэш либо
повторное получение ровно тех же архивов с проверкой идентичности.

Сохранены [итоговое состояние и контрольные суммы](evidence/mvp-final-state.json),
[проверки итогового релиза](evidence/mvp-final-checks.log),
[графическая сессия](evidence/mvp-desktop-session.log),
[Rust tests](evidence/cargo-tests.log), [Clippy](evidence/cargo-clippy.log),
[credentials tests](evidence/credentials-tests.log),
[FAT ENOSPC/publication](evidence/release-failures.log),
[lock failures](evidence/mvp-input-failures.log),
[завершение прерванной сборки](evidence/mvp-recover-b.log),
[загрузка B](evidence/mvp-boot-b.log) и [rollback A](evidence/mvp-rollback-a4.log).
В evidence также есть screenshot desktop-session.png, восстановление пароля
и пустой план текущего desktop-релиза.

Полный перенос каталога evidence отклонён автоматической проверкой из-за
непроверенного содержимого. Вместо него полностью прочитаны 19 выбранных отчётов,
проверено отсутствие managed hashes/crypt hashes/private keys и перенесены только
эти отчёты. Три входных package lock проверены по строгой структуре пакетных полей.
Трассировка pwck и полные install/build logs остались только в защищённой VM.

Console A lock в рабочем примере явно обновлён под окончательный recipe digest;
пакетный состав остался 210. Исходный input lock mvp-a4 сохранён в inputs/mvp-a4.
В итоговом отчёте приведены fingerprints именно фактически собранных релизов.

## Оставшиеся ограничения

Backend пока Python/Bash и ограничен одной VM; Rust обеспечивает декларативный
интерфейс и замораживание входов. Полный граф объяснений зависимостей, миграции,
произвольные аккаунты/локальные каталоги, password aging, полная интеграция
стандартных passwd/GUI, Btrfs ENOSPC/power cuts на всех границах и GC ещё не реализованы.
