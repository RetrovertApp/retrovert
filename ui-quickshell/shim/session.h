#pragma once

#include <QElapsedTimer>
#include <QObject>
#include <QString>
#include <QTimer>
#include <QQmlParserStatus>
#include <QtQml/qqmlregistration.h>

#include "retrovert_ui.h"

// One Rust UI session exposed to QML. Owns the bridge handle, whose audio worker decodes
// in this process; surfaces such as Scope read through it. The timer polls the worker's
// published state at frame rate and raises `frame` when a new capture has landed.
class Session : public QObject, public QQmlParserStatus {
    Q_OBJECT
    Q_INTERFACES(QQmlParserStatus)
    QML_ELEMENT
    Q_PROPERTY(QString pluginDir READ pluginDir WRITE setPluginDir NOTIFY pluginDirChanged)
    Q_PROPERTY(int scopeChannels READ scopeChannels NOTIFY stateChanged)
    Q_PROPERTY(QString status READ status NOTIFY stateChanged)
    Q_PROPERTY(int positionMs READ positionMs NOTIFY stateChanged)
    Q_PROPERTY(QString plugin READ plugin NOTIFY stateChanged)
    Q_PROPERTY(QString error READ error NOTIFY stateChanged)
    Q_PROPERTY(bool running READ running WRITE setRunning NOTIFY runningChanged)

public:
    explicit Session(QObject* parent = nullptr);
    ~Session() override;

    QString pluginDir() const { return m_pluginDir; }
    // Read once when the component completes: the worker loads its decoders at construction,
    // which is why the session is not built while QML is still assigning initial properties.
    void setPluginDir(const QString& dir);
    int scopeChannels() const { return static_cast<int>(m_state.channels); }
    QString status() const;
    int positionMs() const { return static_cast<int>(m_state.position_ms); }
    QString plugin() const { return m_plugin; }
    QString error() const { return m_error; }
    bool running() const { return m_timer.isActive(); }
    void setRunning(bool on);

    void classBegin() override {}
    void componentComplete() override;

    Q_INVOKABLE void open(const QString& path);
    Q_INVOKABLE void stop();

    // Fills `out` with up to `capacity` points of `channel`; returns the count.
    uint32_t scopePoints(uint32_t channel, RvScopePoint* out, uint32_t capacity) const;

signals:
    void pluginDirChanged();
    void stateChanged();
    void runningChanged();
    // Raised once per new frame so every surface schedules one repaint.
    void frame();

private:
    void ensureSession();
    void poll();

    QString m_pluginDir;
    void* m_session = nullptr;
    RvState m_state{};
    QString m_plugin;
    QString m_error;
    QTimer m_timer;
    QElapsedTimer m_clock;
};
