#include <QApplication>
#include <QLineEdit>
#include <QPushButton>
#include <QVBoxLayout>
#include <QWidget>
#include <QString>

int main(int argc, char *argv[]) {
    QApplication app(argc, argv);
    QString title = "Spike Qt6 Fixture";
    for (int i = 1; i + 1 < argc; ++i) {
        if (QString::fromUtf8(argv[i]) == "--spike-title") {
            title = QString::fromUtf8(argv[i + 1]);
        }
    }

    QWidget window;
    window.setWindowTitle(title);
    window.setAccessibleName(title);
    window.resize(420, 220);

    auto *layout = new QVBoxLayout(&window);
    auto *field = new QLineEdit(&window);
    field->setAccessibleName(title + " Input");
    field->setPlaceholderText("Qt6 text input");
    layout->addWidget(field);

    auto *secret = new QLineEdit(&window);
    secret->setEchoMode(QLineEdit::Password);
    secret->setAccessibleName(title + " Password");
    layout->addWidget(secret);

    auto *btn = new QPushButton("Qt6 Action Button", &window);
    btn->setAccessibleName(title + " Button");
    layout->addWidget(btn);

    window.show();
    return app.exec();
}
