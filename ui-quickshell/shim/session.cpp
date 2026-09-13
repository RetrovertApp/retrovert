#include "session.h"

namespace {
constexpr uint32_t kDemoChannels = 4;
constexpr int kPollIntervalMs = 8;
constexpr uint32_t kTextCapacity = 512;

QString readText(uint32_t (*read)(const void*, char*, uint32_t), const void* session) {
    char buf[kTextCapacity];
    const uint32_t n = read(session, buf, kTextCapacity);
    return QString::fromUtf8(buf, static_cast<int>(n));
}
}

Session::Session(QObject* parent) : QObject(parent) {
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

void Session::poll() {
    if (!m_session) return;
    const bool idle = m_state.status == RvStatus_Idle;
    if (idle) rv_ui_demo_tick(m_session, m_clock.elapsed() / 1000.0);

    RvState next{};
    rv_ui_poll(m_session, &next);
    const bool changed = next.channels != m_state.channels || next.status != m_state.status
        || next.position_ms / 1000 != m_state.position_ms / 1000;
    const bool newFrame = next.frame != m_state.frame || next.status == RvStatus_Idle;
    m_state = next;
    if (changed || next.status != RvStatus_Idle) {
        const QString plugin = readText(rv_ui_plugin_name, m_session);
        const QString error = readText(rv_ui_error, m_session);
        if (changed || plugin != m_plugin || error != m_error) {
            m_plugin = plugin;
            m_error = error;
            emit stateChanged();
        }
    }
    if (newFrame) emit frame();
}

uint32_t Session::scopePoints(uint32_t channel, RvScopePoint* out, uint32_t capacity) const {
    return m_session ? rv_ui_scope_points(m_session, channel, out, capacity) : 0;
}
