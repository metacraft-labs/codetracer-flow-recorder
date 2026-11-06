package main

// probe-floor: the lowest height an access node serves register state at,
// measured by bisection, with its two controls.
//
// A replay needs the registers of the state a block started from, so the
// lowest height whose registers an access node returns is the deepest block
// it can replay -- that node's retention, an operational fact of whichever
// node answers rather than a property of the chain. It is measured on one
// register every Flow chain has (the FlowToken account's contract names),
// and the answer is a pair of readings, not one: the floor answers, and the
// height below it is refused with a code distinct from "register absent".
// A node that refused everything, or answered everything, would publish no
// floor at all, and the probe says which happened.

import (
	"context"
	"encoding/json"
	"fmt"
	"os"
	"time"

	"github.com/onflow/flow/protobuf/go/flow/access"
	"github.com/onflow/flow/protobuf/go/flow/entities"
	"github.com/onflow/flow/protobuf/go/flow/executiondata"
	"google.golang.org/grpc"
	"google.golang.org/grpc/codes"
	"google.golang.org/grpc/credentials/insecure"
	"google.golang.org/grpc/status"

	"github.com/onflow/flow-go/engine/common/rpc/convert"
	"github.com/onflow/flow-go/fvm/systemcontracts"
	"github.com/onflow/flow-go/model/flow"
)

// FloorProbe is the probe-floor document.
type FloorProbe struct {
	Schema     string `json:"schema"`
	AccessNode string `json:"accessNode"`
	MeasuredAt string `json:"measuredAt"`
	Chain      string `json:"chain"`
	// The node's own account of itself, beside the measurement.
	NodeVersion     string  `json:"nodeVersion"`
	SporkRootHeight uint64  `json:"sporkRootHeight"`
	NodeRootHeight  uint64  `json:"nodeRootHeight"`
	SealedTip       uint64  `json:"sealedTip"`
	Register        string  `json:"register"`
	Floor           *uint64 `json:"floor"`
	Below           *Probe  `json:"below,omitempty"`
	AtFloor         *Probe  `json:"atFloor,omitempty"`
	AtTip           Probe   `json:"atTip"`
	Probes          int     `json:"probes"`
	Verdict         string  `json:"verdict"`
}

// Probe is one register read at one height, classified.
type Probe struct {
	Height uint64 `json:"height"`
	// served: the node answered with the register's value or its absence;
	// out-of-range: the node refused the height; error: anything else.
	Class   string `json:"class"`
	Code    string `json:"code,omitempty"`
	Message string `json:"message,omitempty"`
}

func runProbeFloor(accessNode string, chainID flow.ChainID, out string) error {
	ctx := context.Background()
	conn, err := grpc.NewClient(accessNode, grpc.WithTransportCredentials(insecure.NewCredentials()))
	if err != nil {
		return err
	}
	defer conn.Close()
	api := access.NewAccessAPIClient(conn)
	data := executiondata.NewExecutionDataAPIClient(conn)

	p := FloorProbe{
		Schema:     "codetracer-flow-recorder/floor-probe@1",
		AccessNode: accessNode,
		MeasuredAt: time.Now().UTC().Format(time.RFC3339),
		Chain:      string(chainID),
	}
	if info, err := api.GetNodeVersionInfo(ctx, &access.GetNodeVersionInfoRequest{}); err == nil {
		p.NodeVersion = info.Info.Semver
		p.SporkRootHeight = info.Info.SporkRootBlockHeight
		p.NodeRootHeight = info.Info.NodeRootBlockHeight
	}
	sealed, err := api.GetLatestBlockHeader(ctx, &access.GetLatestBlockHeaderRequest{IsSealed: true})
	if err != nil {
		return fmt.Errorf("sealed tip: %w", err)
	}
	p.SealedTip = sealed.Block.Height

	flowToken := systemcontracts.SystemContractsForChain(chainID).FlowToken.Address
	reg := &entities.RegisterID{Owner: flowToken.Bytes(), Key: []byte("contract_names")}
	p.Register = flowToken.HexWithPrefix() + "/contract_names"

	read := func(h uint64) Probe {
		p.Probes++
		_, err := data.GetRegisterValues(ctx, &executiondata.GetRegisterValuesRequest{
			BlockHeight: h,
			RegisterIds: []*entities.RegisterID{reg},
		})
		switch status.Code(err) {
		case codes.OK, codes.NotFound:
			return Probe{Height: h, Class: "served"}
		case codes.OutOfRange:
			return Probe{Height: h, Class: "out-of-range", Code: "OutOfRange", Message: status.Convert(err).Message()}
		default:
			return Probe{Height: h, Class: "error", Code: status.Code(err).String(), Message: status.Convert(err).Message()}
		}
	}

	p.AtTip = read(p.SealedTip)
	if p.AtTip.Class != "served" {
		p.Verdict = "no-register-state: the node does not serve registers at its own sealed tip"
		return writeProbe(p, out)
	}
	if bottom := read(0); bottom.Class == "served" {
		zero := uint64(0)
		p.Floor, p.AtFloor = &zero, &bottom
		p.Verdict = "to-genesis: the node serves registers at height 0"
		return writeProbe(p, out)
	} else if bottom.Class != "out-of-range" {
		p.Verdict = "unclassified: height 0 answered " + bottom.Code
		p.Below = &bottom
		return writeProbe(p, out)
	}
	// Invariant: lo is refused out-of-range, hi is served.
	lo, hi := uint64(0), p.SealedTip
	for hi-lo > 1 {
		mid := lo + (hi-lo)/2
		r := read(mid)
		switch r.Class {
		case "served":
			hi = mid
		case "out-of-range":
			lo = mid
		default:
			p.Below = &r
			p.Verdict = "unclassified: height " + fmt.Sprint(mid) + " answered " + r.Code
			return writeProbe(p, out)
		}
	}
	below, at := read(lo), read(hi)
	p.Floor, p.Below, p.AtFloor = &hi, &below, &at
	if below.Class == "out-of-range" && at.Class == "served" {
		p.Verdict = "floor: served at the floor, refused out-of-range one block below"
	} else {
		p.Verdict = "unstable: the boundary did not hold on re-reading"
	}
	return writeProbe(p, out)
}

func writeProbe(p FloorProbe, out string) error {
	data, err := json.MarshalIndent(p, "", "  ")
	if err != nil {
		return err
	}
	data = append(data, '\n')
	if out == "" {
		_, err = os.Stdout.Write(data)
		return err
	}
	return os.WriteFile(out, data, 0o644)
}

// BlockListing is the `block` document: one sealed block and its
// transactions, in execution order, with the three roles each names.
type BlockListing struct {
	Schema       string           `json:"schema"`
	AccessNode   string           `json:"accessNode"`
	Height       uint64           `json:"height"`
	ID           string           `json:"id"`
	ParentID     string           `json:"parentId"`
	Timestamp    string           `json:"timestamp"`
	Transactions []BlockTxListing `json:"transactions"`
}

type BlockTxListing struct {
	ID          string   `json:"id"`
	Proposer    string   `json:"proposer"`
	Payer       string   `json:"payer"`
	Authorizers []string `json:"authorizers"`
	GasLimit    uint64   `json:"gasLimit"`
	// System is true for the protocol's own transactions (the system chunk),
	// which have no payer.
	System bool `json:"system"`
}

func runBlock(accessNode string, chainID flow.ChainID, height uint64, out string) error {
	ctx := context.Background()
	conn, err := grpc.NewClient(accessNode, grpc.WithTransportCredentials(insecure.NewCredentials()),
		grpc.WithDefaultCallOptions(grpc.MaxCallRecvMsgSize(64<<20)))
	if err != nil {
		return err
	}
	defer conn.Close()
	api := access.NewAccessAPIClient(conn)
	hdr, err := api.GetBlockHeaderByHeight(ctx, &access.GetBlockHeaderByHeightRequest{Height: height})
	if err != nil {
		return fmt.Errorf("block %d: %w", height, err)
	}
	txs, err := api.GetTransactionsByBlockID(ctx, &access.GetTransactionsByBlockIDRequest{BlockId: hdr.Block.Id})
	if err != nil {
		return fmt.Errorf("transactions of block %d: %w", height, err)
	}
	listing := BlockListing{
		Schema:     "codetracer-flow-recorder/block@1",
		AccessNode: accessNode,
		Height:     hdr.Block.Height,
		ID:         fmt.Sprintf("%x", hdr.Block.Id),
		ParentID:   fmt.Sprintf("%x", hdr.Block.ParentId),
		Timestamp:  hdr.Block.Timestamp.AsTime().UTC().Format(time.RFC3339Nano),
	}
	for _, tx := range txs.Transactions {
		body, err := convert.MessageToTransaction(tx, chainID.Chain())
		if err != nil {
			return fmt.Errorf("transaction of block %d: %w", height, err)
		}
		entry := BlockTxListing{
			ID:          body.ID().String(),
			Proposer:    body.ProposalKey.Address.HexWithPrefix(),
			Payer:       body.Payer.HexWithPrefix(),
			Authorizers: []string{},
			GasLimit:    body.GasLimit,
			System:      body.Payer == flow.EmptyAddress,
		}
		for _, a := range body.Authorizers {
			entry.Authorizers = append(entry.Authorizers, a.HexWithPrefix())
		}
		listing.Transactions = append(listing.Transactions, entry)
	}
	data, err := json.MarshalIndent(listing, "", "  ")
	if err != nil {
		return err
	}
	data = append(data, '\n')
	if out == "" {
		_, err = os.Stdout.Write(data)
		return err
	}
	return os.WriteFile(out, data, 0o644)
}
