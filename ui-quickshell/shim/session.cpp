#include "session.h"

#include <QJsonDocument>
#include <QJsonObject>

namespace {
constexpr uint32_t kDemoChannels = 4;
constexpr int kPollIntervalMs = 8;
constexpr uint32_t kTextCapacity = 512;
constexpr size_t kDocumentInitialCapacity = 64 * 1024;
// The clock's text and progress rule need no finer step than this.
constexpr int64_t kPositionStepMs = 100;

QString readText(uint32_t (*read)(const void*, char*, uint32_t), const void* session) {
    char buf[kTextCapacity];
    const uint32_t n = read(session, buf, kTextCapacity);
    return QString::fromUtf8(buf, static_cast<int>(n));
}
}

Session::Session(QObject* parent) : QObject(parent), m_document(kDocumentInitialCapacity) {
    m_clock.start();
    m_timer.setInterval(kPollIntervalMs);
    m_timer.setTimerType(Qt::PreciseTimer);
    connect(&m_timer, &QTimer::timeout, this, &Session::poll);
}

Session::~Session() {
    rv_ui_free(m_session);
}

void Session::setPluginDir(const QString& dir) {
    if (dir == m_pluginDir) return;
    m_pluginDir = dir;
    emit pluginDirChanged();
}

void Session::ensureSession() {
    if (m_session) return;
    const QByteArray dir = m_pluginDir.toUtf8();
    m_session = rv_ui_new(dir.constData(), kDemoChannels);
    poll();
}

QString Session::status() const {
    switch (m_state.status) {
    case RvStatus_Playing: return QStringLiteral("playing");
    case RvStatus_Paused: return QStringLiteral("paused");
    case RvStatus_Finished: return QStringLiteral("finished");
    case RvStatus_Error: return QStringLiteral("error");
    default: return QStringLiteral("idle");
    }
}

void Session::componentComplete() {
    ensureSession();
}

void Session::setRunning(bool on) {
    if (on == m_timer.isActive()) return;
    if (on) m_timer.start(); else m_timer.stop();
    emit runningChanged();
}

void Session::open(const QString& path) {
    ensureSession();
    const QByteArray bytes = path.toUtf8();
    rv_ui_open(m_session, bytes.constData());
}

void Session::stop() {
    if (m_session) rv_ui_stop(m_session);
}

void Session::togglePause() {
    if (m_session) rv_ui_toggle_pause(m_session);
}

void Session::seek(int positionMs) {
    if (m_session) rv_ui_seek(m_session, positionMs);
}

void Session::next() {
    if (m_session) rv_ui_next(m_session);
}

void Session::previous() {
    if (m_session) rv_ui_previous(m_session);
}

void Session::setSubsong(int index) {
    if (m_session && index >= 0) rv_ui_set_subsong(m_session, static_cast<uint32_t>(index));
}

void Session::setLoop(int mode) {
    if (m_session && mode >= 0) rv_ui_set_loop(m_session, static_cast<uint32_t>(mode));
}

void Session::setVolume(double volume) {
    if (m_session) rv_ui_set_volume(m_session, static_cast<float>(volume));
}

void Session::clearQueue() {
    if (m_session) rv_ui_clear_queue(m_session);
}

void Session::scanLibrary(const QString& root) {
    ensureSession();
    const QByteArray bytes = root.toUtf8();
    rv_ui_scan_library(m_session, bytes.constData());
}

void Session::playRows(const QVariantList& rows, int start) {
    if (!m_session || rows.isEmpty() || start < 0) return;
    std::vector<uint32_t> ids;
    ids.reserve(static_cast<size_t>(rows.size()));
    for (const QVariant& row : rows) {
        bool ok = false;
        const int id = row.toInt(&ok);
        if (ok && id >= 0) ids.push_back(static_cast<uint32_t>(id));
    }
    if (ids.empty()) return;
    rv_ui_play_rows(m_session, ids.data(), static_cast<uint32_t>(ids.size()),
                    static_cast<uint32_t>(start));
}

QVariantMap Session::readDocument(uint32_t which) {
    uint32_t needed = rv_ui_document(m_session, which, m_document.data(),
                                     static_cast<uint32_t>(m_document.size()));
    if (needed + 1 > m_document.size()) {
        m_document.resize(needed + 1);
        needed = rv_ui_document(m_session, which, m_document.data(),
                                static_cast<uint32_t>(m_document.size()));
    }
    const size_t length = std::min<size_t>(needed, m_document.size() - 1);
    if (length == 0) return {};
    const QJsonDocument doc = QJsonDocument::fromJson(
        QByteArray::fromRawData(m_document.data(), static_cast<int>(length)));
    return doc.object().toVariantMap();
}

void Session::poll() {
    if (!m_session) return;
    const bool idle = m_state.status == RvStatus_Idle;
    if (idle) rv_ui_demo_tick(m_session, m_clock.elapsed() / 1000.0);

    RvState next{};
    rv_ui_poll(m_session, &next);
    const RvState prev = m_state;
    m_state = next;

    const bool stateChangedNow = next.channels != prev.channels || next.status != prev.status
        || next.duration_ms != prev.duration_ms || next.subsong != prev.subsong
        || next.subsong_count != prev.subsong_count || next.loop_mode != prev.loop_mode
        || next.volume != prev.volume || next.library_scanned != prev.library_scanned
        || next.library_total != prev.library_total;
    const bool positionChangedNow = next.position_ms / kPositionStepMs != prev.position_ms / kPositionStepMs
        || next.has_position != prev.has_position || next.order != prev.order
        || next.pattern != prev.pattern || next.row != prev.row;
    const bool newFrame = next.frame != prev.frame || next.status == RvStatus_Idle;

    if (next.status != prev.status || next.status == RvStatus_Error) {
        const QString plugin = readText(rv_ui_plugin_name, m_session);
        const QString error = readText(rv_ui_error, m_session);
        if (plugin != m_plugin || error != m_error) {
            m_plugin = plugin;
            m_error = error;
        }
    }
    if (stateChangedNow) emit stateChanged();
    if (positionChangedNow) emit positionChanged();
    if (next.library_rev != prev.library_rev) {
        m_library = readDocument(RvDocument_Library);
        emit libraryChanged();
    }
    if (next.track_rev != prev.track_rev) {
        m_track = readDocument(RvDocument_Track);
        emit trackChanged();
    }
    if (next.queue_rev != prev.queue_rev) {
        m_queue = readDocument(RvDocument_Queue);
        emit queueChanged();
    }
    if (newFrame) emit frame();
}

uint32_t Session::scopePoints(uint32_t channel, RvScopePoint* out, uint32_t capacity) const {
    return m_session ? rv_ui_scope_points(m_session, channel, out, capacity) : 0;
}

uint32_t Session::vuLevels(float* out, uint32_t capacity) const {
    return m_session ? rv_ui_vu(m_session, out, capacity) : 0;
}
