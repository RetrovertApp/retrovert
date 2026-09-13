#pragma once

#include <QElapsedTimer>
#include <QObject>
#include <QString>
#include <QTimer>
#include <QVariantList>
#include <QVariantMap>
#include <QQmlParserStatus>
#include <QtQml/qqmlregistration.h>
#include <vector>

#include "retrovert_ui.h"

// One Rust UI session exposed to QML. Owns the bridge handle, whose audio worker decodes
// in this process; surfaces such as Scope and Vu read through it. The timer polls the
// worker's published state at frame rate and raises `frame` when a new capture has landed.
//
// Three kinds of change reach QML at three rates: `frame` for the surfaces, `positionChanged`
// for the clock and the tracker position, `stateChanged` for everything that moves at song
// rate. The library, the playing song's metadata and the queue are JSON documents the bridge
// publishes with a revision; each is re-read and parsed only when its revision moves.
class Session : public QObject, public QQmlParserStatus {
    Q_OBJECT
    Q_INTERFACES(QQmlParserStatus)
    QML_ELEMENT
    Q_PROPERTY(QString pluginDir READ pluginDir WRITE setPluginDir NOTIFY pluginDirChanged)
    Q_PROPERTY(bool running READ running WRITE setRunning NOTIFY runningChanged)
    Q_PROPERTY(int scopeChannels READ scopeChannels NOTIFY stateChanged)
    Q_PROPERTY(QString status READ status NOTIFY stateChanged)
    Q_PROPERTY(QString plugin READ plugin NOTIFY stateChanged)
    Q_PROPERTY(QString error READ error NOTIFY stateChanged)
    Q_PROPERTY(int durationMs READ durationMs NOTIFY stateChanged)
    Q_PROPERTY(int subsong READ subsong NOTIFY stateChanged)
    Q_PROPERTY(int subsongCount READ subsongCount NOTIFY stateChanged)
    Q_PROPERTY(int sampleRate READ sampleRate NOTIFY stateChanged)
    Q_PROPERTY(int loopMode READ loopMode NOTIFY stateChanged)
    Q_PROPERTY(double volume READ volume NOTIFY stateChanged)
    Q_PROPERTY(int libraryScanned READ libraryScanned NOTIFY stateChanged)
    Q_PROPERTY(int libraryTotal READ libraryTotal NOTIFY stateChanged)
    Q_PROPERTY(int positionMs READ positionMs NOTIFY positionChanged)
    Q_PROPERTY(bool hasPosition READ hasPosition NOTIFY positionChanged)
    Q_PROPERTY(int order READ order NOTIFY positionChanged)
    Q_PROPERTY(int pattern READ pattern NOTIFY positionChanged)
    Q_PROPERTY(int row READ row NOTIFY positionChanged)
    Q_PROPERTY(int patternChannels READ patternChannels NOTIFY stateChanged)
    Q_PROPERTY(int patternColumns READ patternColumns NOTIFY stateChanged)
    Q_PROPERTY(QVariantMap library READ library NOTIFY libraryChanged)
    Q_PROPERTY(QVariantMap track READ track NOTIFY trackChanged)
    Q_PROPERTY(QVariantMap queue READ queue NOTIFY queueChanged)

public:
    explicit Session(QObject* parent = nullptr);
    ~Session() override;

    QString pluginDir() const { return m_pluginDir; }
    // Read once when the component completes: the worker loads its decoders at construction,
    // which is why the session is not built while QML is still assigning initial properties.
    void setPluginDir(const QString& dir);
    bool running() const { return m_timer.isActive(); }
    void setRunning(bool on);

    int scopeChannels() const { return static_cast<int>(m_state.channels); }
    QString status() const;
    QString plugin() const { return m_plugin; }
    QString error() const { return m_error; }
    int durationMs() const { return static_cast<int>(m_state.duration_ms); }
    int subsong() const { return static_cast<int>(m_state.subsong); }
    int subsongCount() const { return static_cast<int>(m_state.subsong_count); }
    int sampleRate() const { return static_cast<int>(m_state.sample_rate); }
    int loopMode() const { return static_cast<int>(m_state.loop_mode); }
    double volume() const { return m_state.volume; }
    int libraryScanned() const { return static_cast<int>(m_state.library_scanned); }
    int libraryTotal() const { return static_cast<int>(m_state.library_total); }
    int positionMs() const { return static_cast<int>(m_state.position_ms); }
    bool hasPosition() const { return m_state.has_position != 0; }
    int order() const { return static_cast<int>(m_state.order); }
    int pattern() const { return static_cast<int>(m_state.pattern); }
    int row() const { return static_cast<int>(m_state.row); }
    int patternChannels() const { return static_cast<int>(m_state.pattern_channels); }
    int patternColumns() const { return static_cast<int>(m_state.pattern_columns); }
    QVariantMap library() const { return m_library; }
    QVariantMap track() const { return m_track; }
    QVariantMap queue() const { return m_queue; }

    void classBegin() override {}
    void componentComplete() override;

    Q_INVOKABLE void open(const QString& path);
    Q_INVOKABLE void stop();
    Q_INVOKABLE void togglePause();
    Q_INVOKABLE void seek(int positionMs);
    Q_INVOKABLE void next();
    Q_INVOKABLE void previous();
    Q_INVOKABLE void setSubsong(int index);
    Q_INVOKABLE void setLoop(int mode);
    Q_INVOKABLE void setVolume(double volume);
    Q_INVOKABLE void clearQueue();
    Q_INVOKABLE void scanLibrary(const QString& root);
    // Plays library rows in the order given, starting at rows[start].
    Q_INVOKABLE void playRows(const QVariantList& rows, int start);

    // Fills `out` with up to `capacity` points of `channel`; returns the count.
    uint32_t scopePoints(uint32_t channel, RvScopePoint* out, uint32_t capacity) const;
    // Fills `out` with up to `capacity` VU levels; returns the count.
    uint32_t vuLevels(float* out, uint32_t capacity) const;
    // Fills `out` with whole rows `lo..hi` of the pattern window, blank outside it; returns
    // the cell count. The window's shape is in `state()`.
    uint32_t patternCells(uint32_t lo, uint32_t hi, RvGridCell* out, uint32_t capacity) const;
    // The last polled state. Written on the GUI thread in `poll`; a render-thread reader
    // sees it during the synchronisation phase, when the GUI thread is blocked.
    const RvState& state() const { return m_state; }

signals:
    void pluginDirChanged();
    void runningChanged();
    void stateChanged();
    void positionChanged();
    void libraryChanged();
    void trackChanged();
    void queueChanged();
    // Raised when the pattern window's cells were replaced: a new pattern, or none.
    void patternChanged();
    // Raised once per new frame so every surface schedules one repaint.
    void frame();

private:
    void ensureSession();
    void poll();
    QVariantMap readDocument(uint32_t which);

    QString m_pluginDir;
    void* m_session = nullptr;
    RvState m_state{};
    QString m_plugin;
    QString m_error;
    QVariantMap m_library;
    QVariantMap m_track;
    QVariantMap m_queue;
    QTimer m_timer;
    QElapsedTimer m_clock;
    // Grown to the largest document seen and kept, so a scan's republishes do not churn.
    std::vector<char> m_document;
};
