#include <QApplication>
#include <QFile>
#include <QJsonDocument>
#include <QJsonObject>
#include <QLabel>
#include <QLineEdit>
#include <QPushButton>
#include <QString>
#include <QVBoxLayout>
#include <QWidget>

static void dumpState(const QString &path, const QString &title, int clicks, const QString &text, int pwLen) {
    if (path.isEmpty()) return;
    QJsonObject obj;
    obj["title"] = title;
    obj["platform"] = QGuiApplication::platformName();
    obj["clicks"] = clicks;
    obj["entry_text"] = text;
    obj["password_len"] = pwLen;
    QFile f(path + ".tmp");
    if (f.open(QIODevice::WriteOnly | QIODevice::Truncate)) {
        f.write(QJsonDocument(obj).toJson(QJsonDocument::Compact));
        f.close();
        QFile::remove(path);
        f.rename(path);
    }
}

int main(int argc, char *argv[]) {
    QApplication app(argc, argv);
    QString statePath = argc > 1 ? QString::fromUtf8(argv[1]) : QString();
    QString title = argc > 2 ? QString::fromUtf8(argv[2]) : QString("Qt6 E2E Window");

    int clicks = 0;
    QWidget window;
    window.setWindowTitle(title);
    window.setAccessibleName(title);
    window.resize(520, 320);

    auto *layout = new QVBoxLayout(&window);

    auto *btn = new QPushButton("Qt6 Submit", &window);
    btn->setAccessibleName("Qt6 Submit");
    layout->addWidget(btn);

    auto *field = new QLineEdit(&window);
    field->setAccessibleName("Qt6 Text Input");
    field->setPlaceholderText("Qt6 text");
    layout->addWidget(field);

    auto *secret = new QLineEdit(&window);
    secret->setEchoMode(QLineEdit::Password);
    secret->setAccessibleName("Qt6 Password Input");
    layout->addWidget(secret);

    auto *status = new QLabel("Idle", &window);
    status->setAccessibleName("Qt6 Status Idle");
    layout->addWidget(status);

    auto sync = [&]() {
        dumpState(statePath, title, clicks, field->text(), secret->text().length());
    };
    QObject::connect(btn, &QPushButton::clicked, [&]() {
        clicks += 1;
        status->setText(QString("Submitted %1").arg(clicks));
        status->setAccessibleName(QString("Submitted %1").arg(clicks));
        sync();
    });
    QObject::connect(field, &QLineEdit::textChanged, [&](const QString &) { sync(); });
    QObject::connect(secret, &QLineEdit::textChanged, [&](const QString &) { sync(); });

    window.show();
    sync();
    return app.exec();
}
