# MEV Primer

## What is MEV

Imagine a busy sports betting exchange where thousands of people are trying to lock in odds before they change.

Now imagine that one person can see new bets arriving just before they are fully settled. They cannot change the rules, but they can react faster than everyone else.

If they notice that one side of the market is briefly mispriced, they can place a trade that captures the gap before it disappears.

That is the easiest way to think about MEV.

MEV is the value someone can make by reacting to pending or newly ordered activity faster than other participants. It is not magic. It is more like very fast market response in a system where order and timing matter.

In blockchain systems, the key advantage is often not better long-term prediction. It is better short-term reaction.

## Why Arbitrum

Arbitrum was chosen because it gives the bot a better view of market activity than many other places.

Think of it like this:

- **Ethereum mainnet** is like trying to win a race on a crowded highway full of professional drivers with custom-built cars.
- **Base** is more like trading in a room where you cannot see the order book clearly enough to react in time.
- **Arbitrum** is the place where you can at least see the orders being lined up before they are fully filled.

That last part is the sequencer feed.

The sequencer feed is like getting a live look at incoming orders just before they are finalized. That does not guarantee profit, but it gives a real signal to work from. For a small independent system, that matters a lot.

## Flash Loans

A flash loan is best understood as "borrow, trade, repay in one breath."

Here is the simple version:

1. You borrow money for one transaction only.
2. You use that money immediately.
3. You complete the trade sequence.
4. You repay the loan before the transaction ends.
5. If repayment does not happen, the whole transaction is canceled as if it never happened.

So a flash loan is not a normal loan. It does not stay open. It only exists if repayment happens inside the same transaction.

That is why flash loans are so useful for arbitrage. They let a bot act on opportunities that would normally require much more capital.

## Atomic Transactions

Atomic means all-or-nothing.

A good analogy is a vending machine.

Either:

- you insert money, press the button, and receive the item

or:

- the purchase does not complete and the machine gives you back the money

You do not end up in a half-finished state where the machine keeps your money but does not give the item.

That is what an atomic transaction does for a trading bot. Either the full trade completes with repayment, or the entire attempt is rolled back.

## The Arbitrage Opportunity

Here is a concrete example.

- ETH is priced at **$2000** on Uniswap.
- ETH is priced at **$2010** on Camelot.
- The bot borrows **$2000 worth of USDC** through a flash loan.
- It buys **1 ETH** on Uniswap for **$2000**.
- It sells that **1 ETH** on Camelot for **$2010**.
- It repays the borrowed **$2000**.
- It keeps the difference, **$10**, minus gas fees.

If gas costs were $2, the net profit would be $8.

If gas costs were $12, the opportunity would not be worth taking, so the bot should skip it.

That is why `arbx` simulates trades first and checks gas carefully before submitting anything.

## Risks

### Competition from other bots

If many bots see the same opportunity, they race each other. The winner gets the trade. The losers may still spend gas trying.

### Gas cost spikes

Arbitrum has a hidden extra cost that comes from posting transaction data back to Ethereum mainnet. In plain English, the network can suddenly become more expensive than a simple gas estimate suggests. A trade that looked profitable a moment ago can become a bad trade if that extra cost jumps.

### The Timeboost structural disadvantage

Some participants can pay for a speed advantage. That means they get a head start in certain races. A small independent bot must work around that by focusing on opportunities that last long enough to still be available without that advantage.

### Why USDC/ETH was avoided in early testing

USDC/ETH is one of the most competitive markets on Arbitrum. It attracts stronger and faster searchers. Early testing focused on less crowded pairs because the goal was to prove the system safely, not to jump straight into the hardest arena.

## Final takeaway

MEV is easiest to understand as fast reaction to temporary mispricing.

`arbx` tries to do that carefully:

- borrow only for one transaction
- trade only when the numbers work
- cancel automatically when they do not
- keep losses limited mostly to gas

That is the whole idea in plain English.