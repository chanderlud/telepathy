package main

import (
	"context"
	"crypto/rand"
	"fmt"
	"os"
	"os/signal"
	"syscall"
	"time"

	logging "github.com/ipfs/go-log/v2"

	libp2p "github.com/libp2p/go-libp2p"
	"github.com/libp2p/go-libp2p/core/crypto"
	"github.com/libp2p/go-libp2p/core/peer"
	relayv2 "github.com/libp2p/go-libp2p/p2p/protocol/circuitv2/relay"
	pbv2 "github.com/libp2p/go-libp2p/p2p/protocol/circuitv2/pb"
	"github.com/libp2p/go-libp2p/p2p/transport/quicreuse"
	ma "github.com/multiformats/go-multiaddr"
)

const (
	keyFile            = "local_key.pem"
	listenTCP          = "/ip4/0.0.0.0/tcp/40142"
	listenQUIC         = "/ip4/0.0.0.0/udp/40142/quic-v1"
	listenWebTransport = "/ip4/0.0.0.0/udp/40142/quic-v1/webtransport"
	identifyString     = "/telepathy/0.0.1"
)

// logger used by this binary; same logging stack as libp2p
var log = logging.Logger("telepathy/relay")

// logMetricsTracer implements relayv2.MetricsTracer and logs every relay event.
type logMetricsTracer struct{}

func (t *logMetricsTracer) RelayStatus(enabled bool) {
	log.Infof("relay status changed: enabled=%v", enabled)
}

func (t *logMetricsTracer) ConnectionOpened() {
	log.Infof("relay connection opened")
}

func (t *logMetricsTracer) ConnectionClosed(d time.Duration) {
	log.Infof("relay connection closed after %s", d)
}

func (t *logMetricsTracer) ConnectionRequestHandled(status pbv2.Status) {
	log.Infof("relay connection request handled: status=%s", status.String())
}

func (t *logMetricsTracer) ReservationAllowed(isRenewal bool) {
	log.Infof("relay reservation allowed (renewal=%v)", isRenewal)
}

func (t *logMetricsTracer) ReservationClosed(cnt int) {
	log.Infof("relay reservation closed; open_connections=%d", cnt)
}

func (t *logMetricsTracer) ReservationRequestHandled(status pbv2.Status) {
	log.Infof("relay reservation request handled: status=%s", status.String())
}

func (t *logMetricsTracer) BytesTransferred(cnt int) {
	// This can be pretty noisy; drop to Debug if it's too much.
	log.Debugf("relay bytes transferred: %d", cnt)
}

func initLogging() {
	// If user hasn't specified GOLOG_LOG_LEVEL, default to info for everything.
	if _, ok := os.LookupEnv("GOLOG_LOG_LEVEL"); !ok {
		if err := logging.SetLogLevel("*", "info"); err != nil {
			// At this point if logging is broken, the process might as well die loudly.
			panic(fmt.Errorf("failed to set log level: %w", err))
		}
	}
}

func main() {
	initLogging()

	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()

	priv, err := loadOrCreateKey(keyFile)
	if err != nil {
		log.Fatalf("failed to load or create key: %v", err)
	}

	pid, err := peer.IDFromPrivateKey(priv)
	if err != nil {
		log.Fatalf("failed to derive peer id: %v", err)
	}

	h, err := libp2p.New(
		libp2p.Identity(priv),

		libp2p.ListenAddrStrings(
			listenTCP,
			listenQUIC,
			listenWebTransport,
		),

		// Sharing ports
		libp2p.ShareTCPListener(),
		libp2p.QUICReuse(quicreuse.NewConnManager),

		// NAT / reachability
		libp2p.NATPortMap(),
		libp2p.EnableNATService(),
		libp2p.EnableAutoNATv2(),
		libp2p.ForceReachabilityPublic(),

		// Relay v2 server with essentially unbounded resources + logging tracer
		libp2p.EnableRelayService(
			relayv2.WithInfiniteLimits(),
			relayv2.WithMetricsTracer(&logMetricsTracer{}),
		),

		libp2p.Ping(true),
		libp2p.ProtocolVersion(identifyString),
		libp2p.UserAgent(identifyString),
	)
	if err != nil {
		log.Fatalf("failed to create libp2p host: %v", err)
	}
	defer func() {
		if err := h.Close(); err != nil {
			log.Errorf("error while closing host: %v", err)
		}
	}()

	log.Infof("relay peer id: %s", pid)

	for _, addr := range h.Addrs() {
		full := addr.Encapsulate(ma.StringCast("/p2p/" + pid.String()))
		log.Infof("listening on %s", full.String())
	}

	// Block until SIGINT or SIGTERM.
	sigCh := make(chan os.Signal, 1)
	signal.Notify(sigCh, syscall.SIGINT, syscall.SIGTERM)

	select {
	case <-sigCh:
		log.Info("signal received, shutting down")
	case <-ctx.Done():
		log.Info("context canceled, shutting down")
	}
}

func loadOrCreateKey(path string) (crypto.PrivKey, error) {
	data, err := os.ReadFile(path)
	if err == nil {
		// Existing key
		priv, err := crypto.UnmarshalPrivateKey(data)
		if err != nil {
			return nil, fmt.Errorf("unmarshalling existing private key: %w", err)
		}
		log.Infof("loaded existing private key from %s", path)
		return priv, nil
	}
	if !os.IsNotExist(err) {
		return nil, fmt.Errorf("reading key file: %w", err)
	}

	// No key yet: generate a new Ed25519 key and persist it.
	log.Infof("no key found at %s; generating new Ed25519 key", path)
	priv, _, err := crypto.GenerateEd25519Key(rand.Reader)
	if err != nil {
		return nil, fmt.Errorf("generating Ed25519 key: %w", err)
	}

	data, err = crypto.MarshalPrivateKey(priv)
	if err != nil {
		return nil, fmt.Errorf("marshalling private key: %w", err)
	}

	if err := os.WriteFile(path, data, 0o600); err != nil {
		return nil, fmt.Errorf("writing key file: %w", err)
	}

	log.Infof("new private key written to %s", path)
	return priv, nil
}
