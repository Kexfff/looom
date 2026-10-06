# Декларативный MVP на VM

Исторический этап 0.1 с Python/Bash backend. Текущая версия 0.2 полностью
перенесена на Rust: [контракт](native-rust.md),
[журнал](runs/2026-10-06-native-rust/README.md).

Первая реализация схемы v1: Rust CLI проверяет декларацию и замораживает входы;
проверенные Python/Bash механизмы пока выполняют пакетную сборку и операции загрузки.
Это постепенный перенос прототипа, не завершённая замена всех компонентов на Rust.

Все сборки, тесты и проверки этого этапа выполняются **в VM**. На хосте только
редактирование исходников, передача файлов и управление экраном/перезагрузкой VM.
Пакеты никогда не устанавливаются в действующий read-only корень.

Практические результаты: [журнал](runs/2026-10-06-mvp/README.md).
Контракт и будущие возможности: [схема](configuration-proposal.md).

## Реализованный интерфейс

```text
looom check <base.yaml>
looom lock <base.yaml>
looom plan <base.yaml>
looom build <base.yaml> <release-id>
looom status
looom publish <release-id>
looom try <release-id>
looom confirm
looom rollback <release-id>
looom recover
```

`base.lock` лежит рядом с YAML; разные варианты, требующие разных lock, хранятся
в разных каталогах. Примеры: [console A](../configs/mvp/base.yaml),
[console B](../configs/mvp/b/base.yaml), [desktop](../configs/mvp/desktop/base.yaml).
`check` не читает пакетные репозитории. `lock` — отдельная операция с доступом к
репозиториям, которая фиксирует полное замыкание и проверяет подписи через pacman.
`plan` не обновляет версии; отсутствие/несогласованность lock является ошибкой.

План показывает изменения пакетов относительно запущенного корня, изменения
объявленных файлов и units, выбранные модули и постоянные каталоги.
Не все причины транзитивных зависимостей и внутренние зависимости units пока
раскрываются пользователю. Эффективные units и сохранённые vendor-зависимости
также записываются в `/usr/lib/looom/unit-plan.json` собранного корня.

`build` фиксирует байты пользовательских файлов, типизированную декларацию,
lock, рецепты и бинарный CLI в `/var/lib/looom/inputs/<id>`.
Текущий исходный каталог перестаёт быть источником изменений запущенной сборки.
Версии рецептов участвуют в согласованности lock.

Установка выполняется `pacstrap -U` из конкретных зафиксированных архивов:
нет неявного разрешения новых версий внутри сборки. Состав установленного корня
сравнивается с lock до создания read-only снимка. Базы репозиториев сохранены
в отдельном кэше и тоже проверяются по контрольным суммам. Для повторной сборки
нужны архивы, подписи, базы и рецепты; одного JSON lock недостаточно.

`build` не публикует релиз и не перезагружает VM. `publish` сохраняет UKI/меню,
`try` назначает следующую загрузку, а `confirm` проверяет фактически запущенный
корень, ядро, монтирования и обязательные службы. Перезагрузка отдельная:

```bash
sudo looom publish <id>
sudo looom try <id>
sudo systemctl reboot
# Уже после загрузки и проверки:
sudo looom confirm
```

В опубликованном MVP-релизе backend по умолчанию берётся из его собственного
`/usr/lib/looom/source`. Для разработки разрешённой VM можно выбрать переданный
исходный комплект через `LOOOM_BACKEND_ROOT=/var/lib/looom/dev/src`.
Не смешивать новый исходный комплект со старым lock: изменение рецепта требует
явного обновления lock. Получение пакетов, сборка и управление релизами требуют root.

## Повторить сборку на подготовленной VM

Примеры и их входные lock находятся в `/var/lib/looom/dev/src/configs/mvp`.
Текущий установленный CLI использует собственный backend без дополнительных
переменных окружения. Для каждого нового релиза требуется новый уникальный ID:

```bash
sudo looom check /var/lib/looom/dev/src/configs/mvp/base.yaml
sudo looom plan /var/lib/looom/dev/src/configs/mvp/base.yaml
sudo looom build /var/lib/looom/dev/src/configs/mvp/base.yaml console-repeat-1
sudo looom publish console-repeat-1
sudo looom try console-repeat-1
sudo systemctl reboot
# После пробной загрузки:
sudo /var/lib/looom/dev/src/scripts/verify-release.sh console-repeat-1
sudo looom confirm
```

Если поменяли пакетные входы или рецепты, перед `plan` явно выполнить
`sudo looom lock <base.yaml>`. Если нужен именно прежний состав, использовать
сохранённый lock и соответствующие рецепты из `/var/lib/looom/inputs/<id>`.
Изменение только управляемого файла не требует нового пакетного lock.

Для варианта B использовать `configs/mvp/b/base.yaml`, для Plasma —
`configs/mvp/desktop/base.yaml`. Последующие действия одинаковы.
Откат выбирает сохранённый релиз и требует отдельной перезагрузки:

```bash
sudo looom rollback mvp-a4
sudo systemctl reboot
```

Прерывание после полной проверки private build воспроизводится в VM:

```bash
sudo env LOOOM_FAIL_AFTER=build looom build \
  /var/lib/looom/dev/src/configs/mvp/b/base.yaml interrupted-repeat-1
# Ожидается ненулевой код; релиз ещё не опубликован.
sudo looom recover
sudo looom publish interrupted-repeat-1
```

Не выполняйте `confirm` из chroot: подтверждается только реально загруженный корень.
Пакетная воспроизводимость здесь означает зафиксированный состав и входы;
побайтовая идентичность UKI/root разных сборок пока не гарантируется.

## Область первой реализации

Backend намеренно ограничен существующей VM: проверяется виртуализация и MAC.
Раскладка берётся из проверенного bootstrap; инструмент не является установщиком
на произвольный физический диск.

Поддерживаются Arch snapshot, linux/linux-lts, none/Plasma, явные пакеты,
root-owned обычные конфиги в `/etc`, inline content/файловые источники,
явная замена пакетного файла, enabled/disabled/masked units, дополнительные
обязательные проверки units. Неподдерживаемые значения завершают операцию ошибкой.

На этой VM аккаунт фиксирован: codex, UID/GID 1000, wheel, bash;
secret ID `login-codex` и `login-root` отображаются на существующие credential-файлы.
Обязательны существующие постоянные каталоги NetworkManager/system-connections
и looom-local, с root:root/0700. Новые постоянные каталоги/миграции, другие аккаунты,
дополнительные kernel arguments, writable одиночные файлы, imports, AUR,
GC и пользовательские приложения пока не реализованы.

Static units не включаются искусственно: QEMU agent запускается по устройству,
shadow.timer сохраняет штатную vendor-связь с timers.target. Необязательные
vendor wants очищаются; базовые связи systemd/dbus сохранены как часть системного рецепта.

Профиль VM сохраняет явную тестовую политику passwordless sudo и SSH по ключу.
Это не default для будущего рабочего компьютера.

## Credentials

Новый генератор проверяет владельца, тип, права и access ACL, открывает путь
через дескрипторы без следования симлинкам, сериализует операции и создаёт yescrypt
через libxcrypt с фиксированной стоимостью. Legacy SHA-512-crypt поддерживается
в ограниченных параметрах для существующих VM credentials.

Credential-файлы — источник истины; runtime shadow — производное представление.
Журнал `.transaction.json` хранит фазу, а не секрет. Прерывание после сохранения
credential оставляет незавершённую операцию; `generate` повторно формирует runtime
из актуального сохранённого хэша. Успех смены сообщается после генерации.

Это не защита от root или чтения незашифрованного образа VM. Полное управление
aging/постоянной блокировкой аккаунтов, самосмена через PAM и стирание всех копий
plaintext в памяти Python пока не реализованы. Стандартные passwd/usermod/GUI
смена пароля всё ещё не поддерживаются; используется `sudo looom-password codex`.

Штатные pwck/grpck отвергают симлинки. Поэтому shadow.service получает drop-in,
который проверяет реальные `/run/looom/accounts/{shadow,gshadow}` после генерации.

## Прерывание и восстановление

Проверенная сборка до read-only снимка отмечается фазой `configured`.
`looom recover` может завершить такую сборку её замороженным finalizer и сохранить
результат как `validated`, без автоматического включения в меню.
Ранние незавершённые пакетные сборки сохраняются для диагностики.

Проверен ENOSPC при копировании UKI на отдельном настоящем FAT в VM,
прерывание после сохранения UKI и повторная публикация. Это не полноценная проверка
всех аппаратных power-cut границ FAT/grubenv или полного заполнения основной Btrfs.
Поддержка безопасного GC и дальнейшие проверки этих границ остаются отдельным этапом.

## Разработка и тесты только в VM

Toolchain находится в отдельном `@toolchain-20261006`, созданном из bootstrap.
Он монтируется в `/var/lib/looom/dev/root`, исходники — в `/var/lib/looom/dev/src`.
Кэш пакетов общий; действующий read-only корень не получает Rust/compiler пакеты.
Toolchain-копия bootstrap является защищённым материалом VM, а не публикуемым образом.

После перезагрузки рабочие монтирования надо создать снова внутри VM:

```bash
sudo mkdir -p /var/lib/looom/dev/root
sudo mount -o subvol=@toolchain-20261006,rw /dev/vda2 /var/lib/looom/dev/root
sudo mount --bind /var/lib/looom/dev/src /var/lib/looom/dev/root/workspace
sudo mount --bind /var/cache/pacman/pkg /var/lib/looom/dev/root/var/cache/pacman/pkg
sudo arch-chroot /var/lib/looom/dev/root /bin/bash
cd /workspace
cargo build --release --locked
LOOOM_TEST_VM=1 cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
```

Все Python/Shell тесты также запускаются только внутри VM:

```bash
sudo python /var/lib/looom/dev/src/scripts/test-credentials.py -v
sudo python /var/lib/looom/dev/src/scripts/test-release-failures.py
sudo python /var/lib/looom/dev/src/scripts/test-mvp-inputs.py
sudo python /var/lib/looom/dev/src/scripts/test-retained-crypto.py
```

Тесты создают собственные fixtures и тестовый FAT; настоящая ESP не заполняется.
Перед реальной перезагрузкой завершать compiler/chroot процессы и рабочие монтирования.
PAM-тест реального входа использует только временную защищённую credential в VM;
после него обязательно выполнить `test-persistent-password.py restore`.
